import 'dart:async';
import 'dart:convert';
import 'dart:io'
    show
        HttpClient,
        HttpStatus,
        InternetAddress,
        InternetAddressType,
        NetworkInterface,
        Platform;
import 'package:bonsoir/bonsoir.dart';
import 'package:flutter/foundation.dart' show kIsWeb, visibleForTesting;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:permission_handler/permission_handler.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/models/hub.dart' show Hub, HubEndpoint, HubType;
import 'package:rhythm_core/providers/hub_discovery.dart' show DiscoveredHub;
import 'solar_orbit.dart'; // For CelestialColors
import 'success_modal.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show
        RhythmApiException,
        RhythmAuthStatus,
        RhythmAuthApi,
        RhythmConfigApi,
        RhythmDiagnosticsApi;
import '../config/feature_flags.dart';
import '../providers/home_provider.dart';
import '../providers/server_sync_provider.dart';
import '../screens/settings/dialogs/sign_in_modal.dart';
import '../screens/hubs/add_home_flow.dart';
import '../screens/hubs/ble_provisioning_screen.dart';
import '../screens/hubs/hue_configurator_screen.dart';
import '../services/account_cloud_sync_service.dart';
import '../services/analytics_service.dart';
import '../services/auth_service.dart';
import '../services/ble_provisioning_service.dart';
import '../services/local_rhythm_server_service.dart';
import '../services/recent_servers_service.dart';

/// Which empty-state variant to show.
enum ConnectHubMode { rhythmServer, hue }

@visibleForTesting
const int rhythmServerDefaultPort = 54448;

String _rhythmServerEndpointKey(String host, int port) => '$host:$port';

@visibleForTesting
bool rhythmServerEndpointIsConnectingForTesting({
  required String? connectingEndpoint,
  required String host,
  required int port,
}) =>
    connectingEndpoint == _rhythmServerEndpointKey(host, port);

@visibleForTesting
List<AccountHomeServerHubs> rhythmMergedHomeEntriesForTesting({
  required Iterable<AccountHomeServerHubs> localHomes,
  required Iterable<AccountHomeServerHubs> cloudHomes,
}) =>
    _mergeHomeEntries(
      localHomes: localHomes,
      cloudHomes: cloudHomes,
    );

@visibleForTesting
bool rhythmRecentServerIsRepresentedByHomeForTesting({
  required RecentServer server,
  required Iterable<AccountHomeServerHubs> homes,
}) =>
    _recentServerIsRepresentedByHome(server, homes);

@visibleForTesting
bool rhythmDiscoveredServerIsRepresentedByHomeForTesting({
  required DiscoveredHub server,
  required Iterable<AccountHomeServerHubs> homes,
  String? authToken,
}) =>
    _discoveredServerIsRepresentedByHome(
      server,
      homes,
      authToken: authToken,
    );

@visibleForTesting
AccountHomeServerHubs? rhythmHomeEntryForDiscoveredServerForTesting({
  required DiscoveredHub server,
  required Iterable<AccountHomeServerHubs> homes,
  String? authToken,
}) =>
    _homeEntryForDiscoveredServer(
      server: server,
      homes: homes,
      authToken: authToken,
    );

@visibleForTesting
Set<String> rhythmHomeIdsRepresentedByForTesting({
  required AccountHomeServerHubs snapshot,
  required Iterable<AccountHomeServerHubs> localHomes,
  required Iterable<AccountHomeServerHubs> cloudHomes,
}) =>
    _homeIdsRepresentedBy(
      snapshot: snapshot,
      localHomes: localHomes,
      cloudHomes: cloudHomes,
    );

List<AccountHomeServerHubs> _mergeHomeEntries({
  required Iterable<AccountHomeServerHubs> localHomes,
  required Iterable<AccountHomeServerHubs> cloudHomes,
}) {
  final entries = <AccountHomeServerHubs>[
    for (final snapshot in localHomes) _homeEntryForHomeId(snapshot),
  ];

  for (final cloudSnapshot in cloudHomes) {
    final matchIndex = entries.indexWhere(
      (entry) => _homeEntriesRepresentSameHome(entry, cloudSnapshot),
    );
    if (matchIndex == -1) {
      entries.add(cloudSnapshot);
      continue;
    }

    entries[matchIndex] = _mergeHomeEntry(entries[matchIndex], cloudSnapshot);
  }

  return List.unmodifiable(entries);
}

AccountHomeServerHubs _homeEntryForHomeId(AccountHomeServerHubs snapshot) {
  return AccountHomeServerHubs(
    home: snapshot.home,
    serverHubs: [
      for (final hub in snapshot.serverHubs)
        hub.copyWith(homeId: snapshot.home.id),
    ],
  );
}

AccountHomeServerHubs _mergeHomeEntry(
  AccountHomeServerHubs base,
  AccountHomeServerHubs incoming,
) {
  final homeId = base.home.id;
  final hubs = <Hub>[
    for (final hub in base.serverHubs) hub.copyWith(homeId: homeId),
  ];

  for (final hub in incoming.serverHubs) {
    final normalized = hub.copyWith(homeId: homeId);
    final matchIndex = hubs.indexWhere(
      (existing) => _serverHubsRepresentSameBox(existing, normalized),
    );
    if (matchIndex == -1) {
      hubs.add(normalized);
    } else {
      hubs[matchIndex] = _mergeServerHubForHome(
        base: hubs[matchIndex],
        incoming: normalized,
        homeId: homeId,
      );
    }
  }

  return AccountHomeServerHubs(
    home: base.home,
    serverHubs: List.unmodifiable(hubs),
  );
}

Hub _mergeServerHubForHome({
  required Hub base,
  required Hub incoming,
  required String homeId,
}) {
  final baseToken = base.token?.trim();
  final incomingToken = incoming.token?.trim();
  final lastConnected =
      _latestNullableDate(base.lastConnected, incoming.lastConnected);
  final updatedAt = _latestDate(base.updatedAt, incoming.updatedAt);

  return base.copyWith(
    homeId: homeId,
    name: base.name.trim().isNotEmpty ? base.name : incoming.name,
    remoteEndpoint: base.remoteEndpoint ?? incoming.remoteEndpoint,
    token: baseToken != null && baseToken.isNotEmpty
        ? base.token
        : incomingToken != null && incomingToken.isNotEmpty
            ? incoming.token
            : base.token,
    lastConnected: lastConnected,
    updatedAt: updatedAt,
    enabled: base.enabled || incoming.enabled,
  );
}

DateTime _latestDate(DateTime left, DateTime right) {
  return left.isAfter(right) ? left : right;
}

DateTime? _latestNullableDate(DateTime? left, DateTime? right) {
  if (left == null) return right;
  if (right == null) return left;
  return _latestDate(left, right);
}

bool _homeEntriesRepresentSameHome(
  AccountHomeServerHubs left,
  AccountHomeServerHubs right,
) {
  if (left.home.id == right.home.id) return true;
  if (_normalizedHomeName(left.home.name) !=
      _normalizedHomeName(right.home.name)) {
    return false;
  }

  for (final leftHub in left.serverHubs) {
    for (final rightHub in right.serverHubs) {
      if (_serverHubsRepresentSameBox(leftHub, rightHub)) return true;
    }
  }

  return false;
}

String _normalizedHomeName(String name) => name.trim().toLowerCase();

bool _serverHubsRepresentSameBox(Hub left, Hub right) {
  if (left.id == right.id) return true;
  if (_sameHubEndpoint(left.endpoint, right.endpoint)) return true;
  final leftRemote = left.remoteEndpoint;
  final rightRemote = right.remoteEndpoint;
  if (leftRemote != null &&
      rightRemote != null &&
      _sameHubEndpoint(leftRemote, rightRemote)) {
    return true;
  }

  return left.type == HubType.server &&
      right.type == HubType.server &&
      _normalizedHomeName(left.name) == _normalizedHomeName(right.name) &&
      (leftRemote != null || rightRemote != null);
}

bool _sameHubEndpoint(HubEndpoint left, HubEndpoint right) {
  return left.host == right.host &&
      left.port == right.port &&
      left.useSsl == right.useSsl;
}

Set<String> _homeIdsRepresentedBy({
  required AccountHomeServerHubs snapshot,
  required Iterable<AccountHomeServerHubs> localHomes,
  required Iterable<AccountHomeServerHubs> cloudHomes,
}) {
  final ids = <String>{snapshot.home.id};
  for (final entry in [...localHomes, ...cloudHomes]) {
    if (_homeEntriesRepresentSameHome(snapshot, entry)) {
      ids.add(entry.home.id);
    }
  }
  return ids;
}

bool _recentServerIsRepresentedByHome(
  RecentServer server,
  Iterable<AccountHomeServerHubs> homes,
) {
  for (final home in homes) {
    for (final hub in home.serverHubs) {
      if (hub.endpoint.host == server.host &&
          hub.endpoint.port == server.port) {
        return true;
      }
    }
  }
  return false;
}

bool _discoveredServerIsRepresentedByHome(
  DiscoveredHub server,
  Iterable<AccountHomeServerHubs> homes, {
  String? authToken,
}) {
  return _homeEntryForDiscoveredServer(
        server: server,
        homes: homes,
        authToken: authToken,
      ) !=
      null;
}

AccountHomeServerHubs? _homeEntryForDiscoveredServer({
  required DiscoveredHub server,
  required Iterable<AccountHomeServerHubs> homes,
  String? authToken,
}) {
  for (final home in homes) {
    final nextHubs = <Hub>[];
    var matched = false;
    for (final hub in home.serverHubs) {
      if (_serverHubMatchesDiscoveredServer(
        hub,
        server,
        authToken: authToken,
      )) {
        matched = true;
        nextHubs.add(_serverHubForDiscoveredServer(
          hub: hub,
          server: server,
          authToken: authToken,
        ));
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

bool _serverHubMatchesDiscoveredServer(
  Hub hub,
  DiscoveredHub server, {
  String? authToken,
}) {
  if (hub.type != HubType.server) return false;
  if (hub.endpoint.host == server.address && hub.endpoint.port == server.port) {
    return true;
  }

  final cleanAuthToken = authToken?.trim();
  final hubToken = hub.token?.trim();
  return cleanAuthToken != null &&
      cleanAuthToken.isNotEmpty &&
      hubToken != null &&
      hubToken.isNotEmpty &&
      cleanAuthToken == hubToken;
}

Hub _serverHubForDiscoveredServer({
  required Hub hub,
  required DiscoveredHub server,
  required String? authToken,
}) {
  final token = authToken?.trim();
  final name = _displayNameForDiscoveredServer(server);
  return hub.copyWith(
    name: hub.name.trim().isNotEmpty ? hub.name : name,
    endpoint: HubEndpoint(
      host: server.address,
      port: server.port,
      useSsl: false,
    ),
    token: token != null && token.isNotEmpty ? token : hub.token,
    updatedAt: DateTime.now(),
    pendingSync: true,
  );
}

String _displayNameForDiscoveredServer(DiscoveredHub server) {
  final name = server.name?.trim();
  if (name == null || name.isEmpty || name == 'RhythmServer') {
    return 'Rhythm Box';
  }
  return name;
}

class _HomeNameCancelledException implements Exception {
  const _HomeNameCancelledException();
}

const _mdnsProbeTimeout = Duration(milliseconds: 900);
const _mdnsProbeSettleTimeout = Duration(seconds: 2);
const _subnetProbeTimeout = Duration(milliseconds: 250);
const _subnetScanBatchSize = 32;

@visibleForTesting
List<String> rhythmDiscoveryHostCandidates(
  BonsoirService service, {
  bool includeGenericHttpServices = true,
}) {
  final hosts = <String>{};

  void addHost(String? value, {bool requireResolvableShape = false}) {
    final host = _normalizeMdnsHost(value);
    if (host == null) return;
    if (requireResolvableShape && !_hasResolvableHostShape(host)) return;
    hosts.add(host);
  }

  if (includeGenericHttpServices) {
    addHost(service.attributes['ip']);
    addHost(service.host);
    addHost(
      service.attributes['host'],
      requireResolvableShape: true,
    );
  } else if (rhythmMdnsServiceLooksLikeServer(service)) {
    addHost(service.host);
  }

  return List.unmodifiable(hosts);
}

@visibleForTesting
bool rhythmMdnsServiceLooksLikeServer(BonsoirService service) {
  final host = _normalizeMdnsHost(service.host) ?? '';
  final name = service.name.toLowerCase();
  return host.startsWith('rhythm-') || name.contains('rhythm');
}

@visibleForTesting
List<int> rhythmDiscoveryPortCandidates(
  BonsoirService service, {
  bool includeDefaultPort = true,
}) {
  final ports = <int>{};
  if (service.port > 0) ports.add(service.port);
  if (includeDefaultPort) ports.add(rhythmServerDefaultPort);
  return List.unmodifiable(ports);
}

@visibleForTesting
List<String> rhythmSubnetScanCandidates(Iterable<String> localAddresses) {
  final candidates = <String>{};
  for (final address in localAddresses) {
    final octets = _parseIpv4Octets(address);
    if (octets == null || !_isPrivateIpv4(octets)) continue;

    final prefix = '${octets[0]}.${octets[1]}.${octets[2]}.';
    for (var host = 1; host <= 254; host += 1) {
      if (host == octets[3]) continue;
      candidates.add('$prefix$host');
    }
  }
  return List.unmodifiable(candidates);
}

String? _normalizeMdnsHost(String? value) {
  final trimmed = value?.trim();
  if (trimmed == null || trimmed.isEmpty) return null;
  return trimmed.endsWith('.')
      ? trimmed.substring(0, trimmed.length - 1)
      : trimmed;
}

List<int>? _parseIpv4Octets(String value) {
  final parts = value.split('.');
  if (parts.length != 4) return null;

  final octets = <int>[];
  for (final part in parts) {
    final octet = int.tryParse(part);
    if (octet == null || octet < 0 || octet > 255) return null;
    octets.add(octet);
  }
  return octets;
}

bool _isPrivateIpv4(List<int> octets) {
  return octets[0] == 10 ||
      (octets[0] == 172 && octets[1] >= 16 && octets[1] <= 31) ||
      (octets[0] == 192 && octets[1] == 168);
}

bool _hasResolvableHostShape(String host) {
  return InternetAddress.tryParse(host) != null || host.contains('.');
}

/// Full-screen empty state shown when setup is incomplete.
///
/// [ConnectHubMode.rhythmServer] — teal, developer_board icon, opens Rhythm bridge provisioning.
/// [ConnectHubMode.hue] — amber, lightbulb icon, opens Hue configurator.
class ConnectHubScreen extends StatefulWidget {
  final ConnectHubMode mode;

  /// Whether this screen is shown as a modal (with close button) vs inline.
  final bool isModal;
  final bool trackScreenView;

  const ConnectHubScreen(
      {super.key,
      this.mode = ConnectHubMode.rhythmServer,
      this.isModal = false,
      this.trackScreenView = true});

  /// Show as a full-screen modal with slide-up transition and close button.
  static Future<void> show(BuildContext context,
      {ConnectHubMode mode = ConnectHubMode.rhythmServer}) {
    return Navigator.of(context, rootNavigator: true).push(
      PageRouteBuilder(
        opaque: true,
        pageBuilder: (context, animation, secondaryAnimation) {
          return Scaffold(
            backgroundColor: CelestialColors.backgroundDark,
            body: SafeArea(
              child: Stack(
                children: [
                  ConnectHubScreen(mode: mode, isModal: true),
                  Positioned(
                    top: 8,
                    right: 8,
                    child: IconButton(
                      onPressed: () => Navigator.of(context).pop(),
                      icon: Icon(
                        Icons.close,
                        color: CelestialColors.textSecondary
                            .withValues(alpha: 0.7),
                        size: 24,
                      ),
                    ),
                  ),
                ],
              ),
            ),
          );
        },
        transitionsBuilder: (context, animation, secondaryAnimation, child) {
          final curve = CurvedAnimation(
            parent: animation,
            curve: Curves.easeOutCubic,
            reverseCurve: Curves.easeInCubic,
          );
          return SlideTransition(
            position: Tween<Offset>(
              begin: const Offset(0, 1),
              end: Offset.zero,
            ).animate(curve),
            child: child,
          );
        },
        transitionDuration: const Duration(milliseconds: 350),
        reverseTransitionDuration: const Duration(milliseconds: 300),
      ),
    );
  }

  @override
  State<ConnectHubScreen> createState() => _ConnectHubScreenState();
}

class _ConnectHubScreenState extends State<ConnectHubScreen>
    with TickerProviderStateMixin {
  late AnimationController _breatheController;
  late Animation<double> _breathe;

  late AnimationController _rippleController;
  late Animation<double> _ripple;

  // mDNS discovery state (rhythmServer mode only)
  List<DiscoveredHub> _discoveredDevices = [];
  final List<BleDevice> _bleDevices = [];
  bool _isScanning = false;
  bool _isBleScanning = false;
  bool _isConnecting = false;
  String? _connectingEndpoint;
  String? _connectError;
  String? _connectErrorMessage;
  String? _bleScanError;
  BonsoirDiscovery? _bonsoirDiscovery;
  final _bleService = BleProvisioningService();
  StreamSubscription<BleDevice>? _bleScanSubscription;

  // Manual IP state (inline field)
  final _manualIpController = TextEditingController();
  final _manualIpFocus = FocusNode();
  bool _isManualConnecting = false;
  String? _manualConnectError;
  bool _isStartingLocalServer = false;
  String? _localServerError;
  bool _isAddingDevice = false;

  // Account Home state (rhythmServer mode only)
  List<AccountHomeServerHubs> _accountHomes = const [];
  bool _isLoadingAccountHomes = false;
  String? _accountHomesError;
  String? _enteringHomeId;
  String? _deletingHomeId;
  String? _homeActionError;
  StreamSubscription<dynamic>? _authSubscription;

  // RhythmServer branding
  static const _teal = Color(0xFF00BCD4);
  static const _tealDeep = Color(0xFF0097A7);

  // Hue branding
  static const _amber = Color(0xFFFFB900);
  static const _amberDeep = Color(0xFFFF8C00);

  Color get _primary => widget.mode == ConnectHubMode.hue ? _amber : _teal;
  Color get _primaryDeep =>
      widget.mode == ConnectHubMode.hue ? _amberDeep : _tealDeep;
  IconData get _icon => widget.mode == ConnectHubMode.hue
      ? Icons.lightbulb_outline
      : _isAddingDevice
          ? Icons.developer_board
          : Icons.home_rounded;

  String get _title => widget.mode == ConnectHubMode.hue
      ? 'Connect Philips Hue'
      : _isAddingDevice
          ? 'Add a Device'
          : 'Choose Your Home';
  String get _subtitle => widget.mode == ConnectHubMode.hue
      ? 'Connect your Philips Hue bridge to get\nstarted with adaptive lighting'
      : _isAddingDevice
          ? 'Find hardware, then name the Home it belongs to'
          : 'Enter a saved Home or add hardware';
  String get _buttonLabel {
    return 'Connect Philips Hue';
  }

  @override
  void initState() {
    super.initState();

    // Slow breathing glow on the central icon
    _breatheController = AnimationController(
      duration: const Duration(milliseconds: 4000),
      vsync: this,
    )..repeat(reverse: true);
    _breathe = CurvedAnimation(
      parent: _breatheController,
      curve: Curves.easeInOut,
    );

    // Concentric ripple rings expanding outward
    _rippleController = AnimationController(
      duration: const Duration(milliseconds: 4500),
      vsync: this,
    )..repeat();
    _ripple = CurvedAnimation(
      parent: _rippleController,
      curve: Curves.easeOut,
    );

    if (widget.trackScreenView) {
      AnalyticsService().logScreenView('connect_hub');
    }

    // Rebuild when focus state changes so the IP field border reflects it.
    _manualIpFocus.addListener(_onManualIpFocusChanged);

    // Recent servers list — locally cached, rendered immediately even if
    // mDNS hasn't found anything yet. Online/offline overlay refreshes from
    // discovery sweeps via [RecentServersService.markOnline / setOnlineIds].
    if (widget.mode == ConnectHubMode.rhythmServer) {
      RecentServersService.instance.addListener(_onRecentServersChanged);
      _authSubscription = AuthService().authStateChanges.listen((_) {
        if (!mounted) return;
        unawaited(_refreshAccountHomes());
      });
    }

    // Auto-start mDNS scanning in rhythmServer mode
    if (widget.mode == ConnectHubMode.rhythmServer) {
      unawaited(_scanForAllDevices());
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (!mounted) return;
        unawaited(_refreshAccountHomes());
      });
    }
  }

  void _onRecentServersChanged() {
    if (mounted) setState(() {});
  }

  void _onManualIpFocusChanged() {
    if (mounted) setState(() {});
  }

  @override
  void dispose() {
    if (widget.mode == ConnectHubMode.rhythmServer) {
      RecentServersService.instance.removeListener(_onRecentServersChanged);
    }
    _authSubscription?.cancel();
    _bonsoirDiscovery?.stop();
    _bleScanSubscription?.cancel();
    _bleService.dispose();
    _manualIpFocus.removeListener(_onManualIpFocusChanged);
    _manualIpController.dispose();
    _manualIpFocus.dispose();
    _breatheController.dispose();
    _rippleController.dispose();
    super.dispose();
  }

  bool get _canUseAccountHomes =>
      AccountCloudSyncService.instance.canUseSignedInCloudFeatures;

  Future<void> _refreshAccountHomes() async {
    if (widget.mode != ConnectHubMode.rhythmServer) return;
    if (!_canUseAccountHomes) {
      if (!mounted) return;
      setState(() {
        _accountHomes = const [];
        _isLoadingAccountHomes = false;
        _accountHomesError = null;
      });
      return;
    }
    if (_isLoadingAccountHomes) return;

    setState(() {
      _isLoadingAccountHomes = true;
      _accountHomesError = null;
    });

    try {
      final homes = await context.read<HomeProvider>().loadAccountHomes();
      if (!mounted) return;
      setState(() {
        _accountHomes = homes;
        _isLoadingAccountHomes = false;
      });
    } catch (error) {
      debugPrint('Account Homes load failed: $error');
      if (!mounted) return;
      setState(() {
        _accountHomesError = 'Could not load Homes';
        _isLoadingAccountHomes = false;
      });
    }
  }

  Future<void> _scanForAllDevices() async {
    if (widget.mode != ConnectHubMode.rhythmServer) return;
    await Future.wait([
      _scanForDevices().catchError((Object error, StackTrace stackTrace) {
        debugPrint('mDNS scan failed: $error');
      }),
      _scanForBleDevices().catchError((Object error, StackTrace stackTrace) {
        debugPrint('BLE scan failed: $error');
      }),
      // Recents have a known endpoint — probe each directly instead of
      // waiting for mDNS to rediscover them. mDNS only adds *new* boxes
      // to the list.
      _probeRecentServers().catchError((Object error, StackTrace stackTrace) {
        debugPrint('Recent probe failed: $error');
      }),
    ]);
  }

  Future<void> _probeRecentServers() async {
    final recents = RecentServersService.instance.servers;
    if (recents.isEmpty) {
      RecentServersService.instance.setOnlineIds(const []);
      return;
    }

    final results = await Future.wait(
      recents.map((server) async {
        final ok = await _probeRecentServerHealth(server.host, server.port);
        return ok ? server.id : null;
      }),
    );

    if (!mounted) return;
    RecentServersService.instance.setOnlineIds(
      results.whereType<String>(),
    );
  }

  Future<bool> _probeRecentServerHealth(String host, int port) async {
    try {
      return await RhythmDiagnosticsApi(host: host, port: port).healthCheck();
    } catch (_) {
      return false;
    }
  }

  Future<void> _scanForDevices() async {
    setState(() {
      _isScanning = true;
      _discoveredDevices = [];
      _connectingEndpoint = null;
      _connectError = null;
      _connectErrorMessage = null;
    });

    final found = <DiscoveredHub>[];

    if (kIsWeb) {
      await _scanViaWebApi(found);
    } else {
      await _scanViaBonsoir(found);
    }

    if (mounted) {
      AnalyticsService().logMdnsScanCompleted(found.length);
      setState(() {
        _isScanning = false;
      });
    }
  }

  Future<void> _scanForBleDevices() async {
    if (!_supportsBleDiscovery) return;

    await _bleScanSubscription?.cancel();
    _bleService.stopScan();

    final granted = await _ensureBluetoothPermissionForDiscovery();
    if (!granted || !mounted) return;

    if (!Platform.isIOS) {
      final bluetoothOn = await _bleService.isBluetoothOn();
      if (!mounted) return;
      if (!bluetoothOn) {
        setState(() {
          _isBleScanning = false;
          _bleScanError = 'Bluetooth is off';
        });
        return;
      }
    }

    setState(() {
      _isBleScanning = true;
      _bleScanError = null;
      _bleDevices.clear();
    });

    _bleScanSubscription = _bleService.scanForDevices().listen(
      (device) {
        if (!mounted) return;
        final exists =
            _bleDevices.any((candidate) => candidate.id == device.id);
        if (exists) return;
        setState(() {
          _bleDevices.add(device);
        });
      },
      onError: (error) {
        if (!mounted) return;
        setState(() {
          _isBleScanning = false;
          _bleScanError = _bleDiscoveryErrorMessage(error);
        });
      },
    );

    try {
      await _bleService.waitForScanToFinish();
    } catch (_) {
      // Scan lifecycle errors are surfaced through the stream when actionable.
    }
    if (!mounted) return;
    setState(() {
      _isBleScanning = false;
    });
  }

  bool get _supportsBleDiscovery => !kIsWeb;

  Future<bool> _ensureBluetoothPermissionForDiscovery() async {
    if (kIsWeb || Platform.isMacOS || Platform.isIOS) return true;

    final permissions = <Permission>[];
    if (Platform.isAndroid) {
      permissions.addAll([
        Permission.bluetoothScan,
        Permission.bluetoothConnect,
        Permission.locationWhenInUse,
      ]);
    }
    if (permissions.isEmpty) return true;

    final statuses = await permissions.request();
    final denied =
        statuses.entries.where((entry) => !entry.value.isGranted).toList();
    if (denied.isEmpty) return true;

    if (mounted) {
      setState(() {
        _isBleScanning = false;
        _bleScanError = 'Bluetooth permission needed';
      });
    }
    return false;
  }

  String _bleDiscoveryErrorMessage(Object error) {
    final message = '$error';
    if (message.contains('PoweredOff') ||
        message.contains('BluetoothAdapterState.off')) {
      return 'Bluetooth is off';
    }
    if (message.contains('Unauthorized') ||
        message.contains('BluetoothAdapterState.unauthorized')) {
      return 'Bluetooth permission needed';
    }
    if (message.contains('BluetoothAdapterState.unavailable')) {
      return 'Bluetooth unavailable';
    }
    return 'Bluetooth scan failed';
  }

  /// Web: call GET api/discover on the same origin (rhythm-server / addon ingress).
  Future<void> _scanViaWebApi(List<DiscoveredHub> found) async {
    try {
      final base = Uri.base.toString();
      final baseUrl = base.endsWith('/') ? base : '$base/';
      final devices = await RhythmConfigApi(baseUrl: baseUrl).discover();
      for (final device in devices) {
        final hub = DiscoveredHub(
          host: device['host'] as String? ?? '',
          port: device['port'] as int? ?? 80,
          address: device['address'] as String? ?? '',
          name: device['name'] as String? ?? '',
          type: HubType.server,
        );
        if (hub.address.isNotEmpty) {
          found.add(hub);
          if (mounted) {
            setState(() {
              _discoveredDevices = List.of(found);
            });
          }
        }
      }
    } catch (e) {
      debugPrint('Web discover error: $e');
    }
  }

  /// Native: use Bonsoir (mDNS) to discover devices.
  Future<void> _scanViaBonsoir(List<DiscoveredHub> found) async {
    final seen = <String>{};
    final pendingProbes = <Future<void>>{};
    try {
      final discovery = BonsoirDiscovery(type: '_http._tcp');
      _bonsoirDiscovery = discovery;
      await discovery.initialize();

      void queueProbe(BonsoirService service) {
        final probe = _handleResolvedService(service, seen, found)
            .catchError((Object e, StackTrace stackTrace) {
          debugPrint('mDNS: probe failed for ${service.name}: $e');
        });
        pendingProbes.add(probe);
        unawaited(probe.whenComplete(() => pendingProbes.remove(probe)));
      }

      discovery.eventStream?.listen((event) {
        switch (event) {
          case BonsoirDiscoveryServiceFoundEvent():
            event.service.resolve(discovery.serviceResolver).catchError((e) {
              debugPrint('mDNS: resolve failed for ${event.service.name}: $e');
            });
          case BonsoirDiscoveryServiceResolvedEvent():
            queueProbe(event.service);
          case BonsoirDiscoveryServiceUpdatedEvent():
            queueProbe(event.service);
          default:
            break;
        }
      }, onError: (e) {
        if (_isIgnorableBonsoirResolveError(e)) {
          debugPrint('mDNS: ignoring transient Bonsoir resolve error: $e');
          return;
        }
        debugPrint('mDNS: discovery stream error: $e');
      });

      await discovery.start();
      await Future.delayed(const Duration(seconds: 5));
      await discovery.stop();
      final probes = List<Future<void>>.of(pendingProbes);
      if (probes.isNotEmpty) {
        try {
          await Future.wait(probes).timeout(_mdnsProbeSettleTimeout);
        } on TimeoutException {
          debugPrint('mDNS: timed out waiting for health probes');
        }
      }
      if (Platform.isAndroid && found.isEmpty) {
        await _scanLocalSubnetForRhythmServers(found, seen);
      }
      _bonsoirDiscovery = null;
    } catch (e) {
      debugPrint('mDNS scan error: $e');
    }
  }

  bool _isIgnorableBonsoirResolveError(Object error) {
    return error is PlatformException &&
        error.code == 'discoveryError' &&
        (error.message == 'discoveryServiceResolveFailed' ||
            error.message == 'discoveryTxtResolveFailed');
  }

  Future<void> _handleResolvedService(
    BonsoirService service,
    Set<String> seen,
    List<DiscoveredHub> found,
  ) async {
    final hosts = rhythmDiscoveryHostCandidates(
      service,
      includeGenericHttpServices: Platform.isAndroid,
    );
    if (hosts.isEmpty) return;

    final ports = rhythmDiscoveryPortCandidates(
      service,
      includeDefaultPort: Platform.isAndroid,
    );
    for (final host in hosts) {
      final ip = await _resolveMdnsHost(host);
      if (ip == null) continue;

      for (final port in ports) {
        final key = '$ip:$port';
        if (seen.contains(key)) continue;
        seen.add(key);

        final isHealthy = await _checkDiscoveredRhythmServer(ip, port);

        if (isHealthy && mounted) {
          final hub = DiscoveredHub(
            host: host,
            port: port,
            address: ip,
            name: _friendlyMdnsServiceName(service),
            type: HubType.server,
          );
          found.add(hub);
          setState(() {
            _discoveredDevices = List.of(found);
          });
          return;
        }
      }
    }
  }

  Future<void> _scanLocalSubnetForRhythmServers(
    List<DiscoveredHub> found,
    Set<String> seen,
  ) async {
    final localAddresses = await _localIpv4Addresses();
    final candidates = rhythmSubnetScanCandidates(localAddresses);
    if (candidates.isEmpty) return;

    debugPrint('mDNS: scanning local subnet for Rhythm servers');
    for (var index = 0;
        index < candidates.length && mounted;
        index += _subnetScanBatchSize) {
      final batch = candidates.skip(index).take(_subnetScanBatchSize).toList();
      final results = await Future.wait(
        batch.map((ip) async {
          final key = '$ip:$rhythmServerDefaultPort';
          if (seen.contains(key)) return null;
          seen.add(key);

          final isHealthy = await _isRhythmServerHealthy(
            host: ip,
            port: rhythmServerDefaultPort,
            timeout: _subnetProbeTimeout,
          );
          return isHealthy ? ip : null;
        }),
      );

      final healthyIps = results.whereType<String>().toList();
      if (healthyIps.isEmpty) continue;
      for (final ip in healthyIps) {
        found.add(
          DiscoveredHub(
            host: ip,
            port: rhythmServerDefaultPort,
            address: ip,
            name: 'RhythmServer',
            type: HubType.server,
          ),
        );
      }
      setState(() {
        _discoveredDevices = List.of(found);
      });
      return;
    }
  }

  Future<bool> _checkDiscoveredRhythmServer(String host, int port) {
    if (Platform.isAndroid) {
      return _isRhythmServerHealthy(
        host: host,
        port: port,
        timeout: _mdnsProbeTimeout,
      );
    }
    return RhythmDiagnosticsApi(host: host, port: port).healthCheck();
  }

  Future<List<String>> _localIpv4Addresses() async {
    try {
      final interfaces = await NetworkInterface.list(
        includeLoopback: false,
        type: InternetAddressType.IPv4,
      );
      return [
        for (final interface in interfaces)
          for (final address in interface.addresses) address.address,
      ];
    } catch (e) {
      debugPrint('mDNS: could not list local network interfaces: $e');
      return const [];
    }
  }

  Future<bool> _isRhythmServerHealthy({
    required String host,
    required int port,
    required Duration timeout,
  }) async {
    final client = HttpClient()..connectionTimeout = timeout;
    try {
      final request = await client
          .getUrl(Uri.parse('http://$host:$port/health'))
          .timeout(timeout);
      final response = await request.close().timeout(timeout);
      if (response.statusCode != HttpStatus.ok) {
        await response.drain();
        return false;
      }

      final body = await utf8.decoder.bind(response).join().timeout(timeout);
      final payload = jsonDecode(body);
      return payload is Map && payload['status'] == 'healthy';
    } catch (_) {
      return false;
    } finally {
      client.close(force: true);
    }
  }

  Future<String?> _resolveMdnsHost(String host) async {
    final literalAddress = InternetAddress.tryParse(host);
    if (literalAddress != null) return literalAddress.address;

    try {
      final addresses = await InternetAddress.lookup(host);
      for (final address in addresses) {
        if (address.type == InternetAddressType.IPv4) {
          return address.address;
        }
      }
      if (addresses.isNotEmpty) return addresses.first.address;
    } catch (_) {
      debugPrint('mDNS: Could not resolve $host to IP');
    }
    return null;
  }

  String _friendlyMdnsServiceName(BonsoirService service) {
    final attrHost = service.attributes['host']?.trim();
    if (attrHost != null && attrHost.isNotEmpty) return attrHost;
    return service.name.isNotEmpty ? service.name : 'RhythmServer';
  }

  String _discoveredEndpointKey(DiscoveredHub hub) =>
      _rhythmServerEndpointKey(hub.address, hub.port);

  /// Submit the inline manual-IP form.
  Future<void> _submitManualIp() async {
    final input = _manualIpController.text.trim();
    if (input.isEmpty) return;

    // Parse IP:port (default 54448)
    String ip;
    int port;
    if (input.contains(':')) {
      final parts = input.split(':');
      ip = parts[0];
      port = int.tryParse(parts[1]) ?? rhythmServerDefaultPort;
    } else {
      ip = input;
      port = rhythmServerDefaultPort;
    }

    _manualIpFocus.unfocus();
    setState(() {
      _isManualConnecting = true;
      _manualConnectError = null;
    });

    final healthy =
        await RhythmDiagnosticsApi(host: ip, port: port).healthCheck();

    if (!mounted) return;
    if (!healthy) {
      setState(() {
        _isManualConnecting = false;
        _manualConnectError = 'Could not reach $ip:$port';
      });
      return;
    }

    setState(() => _isManualConnecting = false);
    final hub = DiscoveredHub(
      host: ip,
      port: port,
      address: ip,
      name: 'RhythmServer',
      type: HubType.server,
    );
    await _connectToDevice(hub);
  }

  Future<void> _connectToDevice(DiscoveredHub hub) async {
    AnalyticsService().logRhythmServerDiscoveredConnect(hub.address);
    if (!kIsWeb) HapticFeedback.mediumImpact();
    final endpointKey = _discoveredEndpointKey(hub);
    setState(() {
      _isConnecting = true;
      _connectingEndpoint = endpointKey;
      _connectError = null;
      _connectErrorMessage = null;
    });

    String? authToken;
    try {
      authToken = await _resolveAuthTokenForDiscoveredHub(hub);
    } on _AuthTokenRequiredException catch (error) {
      if (!mounted) return;
      setState(() {
        _isConnecting = false;
        _connectingEndpoint = null;
        _connectError = endpointKey;
        _connectErrorMessage = error.message;
      });
      return;
    } catch (error) {
      debugPrint('Auth resolution failed for ${hub.address}: $error');
      if (!mounted) return;
      setState(() {
        _isConnecting = false;
        _connectingEndpoint = null;
        _connectError = endpointKey;
        _connectErrorMessage = 'Could not authorize this Box';
      });
      return;
    }

    final client = RhythmDiagnosticsApi(
      host: hub.address,
      port: hub.port,
      authToken: authToken,
    );
    final isHealthy = await client.healthCheck();

    if (!mounted) return;

    if (isHealthy) {
      if (!LocalRhythmServerService.instance
          .isLocalEndpoint(hub.address, hub.port)) {
        try {
          await LocalRhythmServerService.instance.stop();
        } catch (error) {
          debugPrint('Local Rhythm Server stop failed: $error');
        }
      }

      final Hub? result;
      try {
        result = await _persistDiscoveredServerHub(hub, authToken);
      } on _HomeNameCancelledException {
        if (!mounted) return;
        setState(() {
          _isConnecting = false;
          _connectingEndpoint = null;
        });
        return;
      }

      if (!mounted) return;

      if (result == null) {
        setState(() {
          _isConnecting = false;
          _connectingEndpoint = null;
          _connectError = endpointKey;
          _connectErrorMessage = 'Could not save this Box';
        });
        return;
      }

      // Local recent list — populated regardless of how we got here (mDNS,
      // manual IP, or recent re-tap) so reconnects don't have to wait on a
      // fresh mDNS sweep next time.
      await RecentServersService.instance.record(
        name: result.name,
        host: hub.address,
        port: hub.port,
        token: authToken ?? result.token,
      );

      if (!mounted) return;

      final homeName = context.read<HomeProvider>().currentHome?.name ?? 'Home';
      final serverSync = context.read<ServerSyncProvider>();
      serverSync.beginHomeEntryRefresh(homeName: homeName);
      unawaited(serverSync.refreshForHomeEntry(homeName: homeName));

      if (!kIsWeb) HapticFeedback.heavyImpact();
      await SuccessModal.show(
        context,
        roomCount: 0,
        hubType: 'RhythmServer',
      );

      if (!mounted) return;
      setState(() {
        _isConnecting = false;
        _connectingEndpoint = null;
      });

      if (mounted && widget.isModal) {
        Navigator.of(context).pop();
      }
    } else {
      setState(() {
        _isConnecting = false;
        _connectingEndpoint = null;
        _connectError = endpointKey;
        _connectErrorMessage = 'Could not reach this Box';
      });
    }
  }

  Future<Hub?> _persistDiscoveredServerHub(
    DiscoveredHub discovered,
    String? authToken,
  ) async {
    final homeProvider = context.read<HomeProvider>();
    final existingHome = _homeEntryForDiscoveredServer(
      server: discovered,
      homes: _visibleHomeEntries(homeProvider),
      authToken: authToken,
    );
    if (existingHome != null) {
      return homeProvider.enterHome(existingHome);
    }

    final name = _displayNameForDiscoveredServer(discovered);
    if (!mounted) throw const _HomeNameCancelledException();
    final home = await AddHomeFlow.show(context, defaultName: name);
    if (home == null) throw const _HomeNameCancelledException();

    return homeProvider.addServerHubInNewHome(
      homeName: home.name,
      hubName: name,
      host: discovered.address,
      port: discovered.port,
      token: authToken,
      location: home.location,
      timezone: home.timezone,
    );
  }

  Future<void> _enterHome(AccountHomeServerHubs snapshot) async {
    if (_enteringHomeId != null) return;
    if (!kIsWeb) HapticFeedback.mediumImpact();

    setState(() {
      _enteringHomeId = snapshot.home.id;
      _homeActionError = null;
    });

    final serverSync = context.read<ServerSyncProvider>();
    if (snapshot.hasServerHub) {
      serverSync.beginHomeEntryRefresh(homeName: snapshot.home.name);
    }

    final result = await context.read<HomeProvider>().enterHome(snapshot);
    if (!mounted) return;

    if (snapshot.hasServerHub && result == null) {
      serverSync.cancelHomeEntryRefresh(
        error: 'Could not enter ${snapshot.home.name}',
      );
      setState(() {
        _enteringHomeId = null;
        _homeActionError = 'Could not enter ${snapshot.home.name}';
      });
      return;
    }

    if (snapshot.hasServerHub) {
      unawaited(
        serverSync.refreshForHomeEntry(homeName: snapshot.home.name),
      );
    }

    setState(() {
      _enteringHomeId = null;
      _homeActionError = null;
    });

    if (mounted && widget.isModal) {
      Navigator.of(context).pop();
    }
  }

  Future<void> _startLocalRhythmServer() async {
    if (_isStartingLocalServer) return;
    if (!kIsWeb) HapticFeedback.mediumImpact();

    setState(() {
      _isStartingLocalServer = true;
      _localServerError = null;
    });

    try {
      await LocalRhythmServerService.instance.start(
        port: localRhythmServerDefaultPort,
      );

      final hub = DiscoveredHub(
        host: localRhythmServerHost,
        port: localRhythmServerDefaultPort,
        address: localRhythmServerHost,
        name: 'This Mac',
        type: HubType.server,
      );
      final Hub? result;
      try {
        result = await _persistDiscoveredServerHub(hub, null);
      } on _HomeNameCancelledException {
        if (!mounted) return;
        setState(() => _isStartingLocalServer = false);
        return;
      }
      if (!mounted) return;

      if (result == null) {
        setState(() {
          _isStartingLocalServer = false;
          _localServerError = 'Could not save the local server';
        });
        return;
      }

      await RecentServersService.instance.record(
        name: result.name,
        host: localRhythmServerHost,
        port: localRhythmServerDefaultPort,
        token: result.token,
      );

      if (!mounted) return;
      final homeName = context.read<HomeProvider>().currentHome?.name ?? 'Home';
      final serverSync = context.read<ServerSyncProvider>();
      serverSync.beginHomeEntryRefresh(homeName: homeName);
      unawaited(serverSync.refreshForHomeEntry(homeName: homeName));

      setState(() => _isStartingLocalServer = false);
      if (!kIsWeb) HapticFeedback.heavyImpact();
      await SuccessModal.show(
        context,
        roomCount: 0,
        hubType: 'RhythmServer',
      );

      if (mounted && widget.isModal) {
        Navigator.of(context).pop();
      }
    } catch (error) {
      debugPrint('Local Rhythm Server start failed: $error');
      if (!mounted) return;
      setState(() {
        _isStartingLocalServer = false;
        _localServerError = 'Could not start the local server';
      });
    }
  }

  Future<String?> _resolveAuthTokenForDiscoveredHub(DiscoveredHub hub) async {
    final baseUrl = 'http://${hub.address}:${hub.port}';
    final status = await _serverAuthStatus(baseUrl, hub.address);
    final shouldClaimToken = status != null &&
        status.claimAvailable &&
        (status.requiresAuth || FeatureFlags.remoteAccessTunnel);
    if (status?.requiresAuth != true && !shouldClaimToken) return null;

    final storedToken = await _storedServerTokenFor(hub, baseUrl: baseUrl);
    if (storedToken != null) {
      return storedToken;
    }

    if (shouldClaimToken) {
      final claim = await RhythmAuthApi(baseUrl: baseUrl).claimOwnerToken();
      return claim.token;
    }

    return _requestOwnerTokenViaBle(hub);
  }

  Future<RhythmAuthStatus?> _serverAuthStatus(
    String baseUrl,
    String address,
  ) async {
    try {
      final status = await RhythmAuthApi(baseUrl: baseUrl).getStatus();
      return status;
    } catch (error) {
      debugPrint('Auth status unavailable for $address: $error');
      return null;
    }
  }

  Future<String?> _storedServerTokenFor(
    DiscoveredHub hub, {
    required String baseUrl,
  }) async {
    final exactTokens = <String>[];
    final fallbackTokens = <String>[];

    void addToken(String? token, {required bool exact}) {
      final clean = token?.trim();
      if (clean == null || clean.isEmpty) return;
      final target = exact ? exactTokens : fallbackTokens;
      if (!exactTokens.contains(clean) && !fallbackTokens.contains(clean)) {
        target.add(clean);
      }
    }

    final homeProvider = context.read<HomeProvider>();
    final localServerHubs = <Hub>[
      for (final snapshot in homeProvider.homeServerHubSnapshots)
        ...snapshot.serverHubs,
      ...homeProvider.currentHomeHubs
          .where((hub) => hub.type == HubType.server),
    ];
    for (final savedHub in localServerHubs) {
      if (savedHub.type != HubType.server) continue;
      addToken(
        savedHub.token,
        exact: savedHub.endpoint.host == hub.address &&
            savedHub.endpoint.port == hub.port,
      );
    }

    for (final recent in RecentServersService.instance.servers) {
      addToken(
        recent.token,
        exact: recent.host == hub.address && recent.port == hub.port,
      );
    }

    for (final token in [...exactTokens, ...fallbackTokens]) {
      try {
        await RhythmConfigApi(
          baseUrl: '$baseUrl/',
          authToken: token,
        ).getState().timeout(const Duration(seconds: 4));
        return token;
      } on RhythmApiException catch (error) {
        if (error.statusCode != HttpStatus.unauthorized) {
          debugPrint('Stored token probe failed for ${hub.address}: $error');
        }
      } catch (error) {
        debugPrint('Stored token probe failed for ${hub.address}: $error');
      }
    }

    return null;
  }

  Future<String> _requestOwnerTokenViaBle(DiscoveredHub hub) async {
    var device = _matchingBleDeviceFor(hub);
    if (device == null && !_isBleScanning) {
      await _scanForBleDevices().timeout(
        const Duration(seconds: 6),
        onTimeout: () {},
      );
      device = _matchingBleDeviceFor(hub);
    }

    if (device == null) {
      throw const _AuthTokenRequiredException('Bluetooth auth needed');
    }

    setState(() {
      _isBleScanning = false;
      _bleScanError = null;
    });

    try {
      await _bleService.connectForAuth(device);
      return await _bleService.requestOwnerToken(label: 'Rhythm app');
    } finally {
      await _bleService.disconnect();
    }
  }

  BleDevice? _matchingBleDeviceFor(DiscoveredHub hub) {
    if (_bleDevices.isEmpty) return null;
    final hubTokens = _normalizedDiscoveryTokens([
      hub.name,
      hub.host,
      hub.address,
    ]);
    for (final device in _bleDevices) {
      final deviceTokens = _normalizedDiscoveryTokens([device.name]);
      if (hubTokens.intersection(deviceTokens).isNotEmpty) return device;
    }
    if (_bleDevices.length == 1) return _bleDevices.single;
    return null;
  }

  Set<String> _normalizedDiscoveryTokens(Iterable<String?> values) {
    final tokens = <String>{};
    for (final value in values) {
      final normalized = value
          ?.toLowerCase()
          .replaceAll('.local', '')
          .replaceAll(RegExp(r'[^a-z0-9]+'), ' ')
          .trim();
      if (normalized == null || normalized.isEmpty) continue;
      tokens.addAll(normalized.split(' ').where((token) => token.length >= 4));
      tokens.add(normalized.replaceAll(' ', ''));
    }
    tokens.removeWhere((token) => token == 'rhythm' || token == 'server');
    return tokens;
  }

  void _openConfigurator() {
    AnalyticsService().logRhythmServerSetupTapped(
      widget.mode == ConnectHubMode.hue ? 'hue' : 'rhythmServer',
    );
    HapticFeedback.mediumImpact();
    if (widget.mode == ConnectHubMode.hue) {
      HueConfiguratorScreen.show(context);
    }
  }

  Future<void> _openBleProvisioning([BleDevice? initialDevice]) async {
    AnalyticsService().logRhythmServerSetupTapped('ble');
    HapticFeedback.mediumImpact();
    await BleProvisioningScreen.show(context, initialDevice: initialDevice);
  }

  void _openAddDeviceScreen() {
    if (!kIsWeb) HapticFeedback.selectionClick();
    setState(() {
      _isAddingDevice = true;
      _homeActionError = null;
    });
    unawaited(_scanForAllDevices());
  }

  void _returnToHomeChooser() {
    if (!kIsWeb) HapticFeedback.selectionClick();
    _manualIpFocus.unfocus();
    setState(() {
      _isAddingDevice = false;
      _manualConnectError = null;
      _connectError = null;
      _connectErrorMessage = null;
    });
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: Listenable.merge([
        _breatheController,
        _rippleController,
      ]),
      builder: (context, _) {
        final isServer = widget.mode == ConnectHubMode.rhythmServer;
        final keyboardOpen = MediaQuery.of(context).viewInsets.bottom > 0;
        final bottomClearance = isServer ? 112.0 : 88.0;

        return Padding(
          padding: EdgeInsets.only(
            top: widget.isModal ? 24 : 8,
            bottom: bottomClearance,
          ),
          child: isServer
              ? _buildRhythmServerLayout(keyboardOpen)
              : _buildHueLayout(),
        );
      },
    );
  }

  // ── Layouts ────────────────────────────────────────────────────────────────

  Widget _buildRhythmServerLayout(bool keyboardOpen) {
    // Keep the IP TextField rendered in one stable spot so focus survives.
    // Only peripheral sections (hero, scan pill, new-box setup) collapse
    // away when the keyboard is up, giving the field room to lift.
    return Column(
      children: [
        if (!keyboardOpen) ...[
          if (_isAddingDevice) _buildAddDeviceBackButton(),
          _buildHeroSection(),
          const SizedBox(height: 8),
        ],
        _buildTitleAndStatus(),
        const SizedBox(height: 10),
        if (!keyboardOpen && _isAddingDevice) ...[
          _buildScanButton(),
          const SizedBox(height: 10),
        ],
        Expanded(child: _buildMiddleContent()),
        if (_isAddingDevice) ...[
          const SizedBox(height: 10),
          _buildBottomActions(keyboardOpen),
          const SizedBox(height: 4),
        ],
      ],
    );
  }

  Widget _buildAddDeviceBackButton() {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 16),
      child: Align(
        alignment: Alignment.centerLeft,
        child: IconButton(
          tooltip: 'Back to Homes',
          onPressed: _returnToHomeChooser,
          icon: Icon(
            Icons.arrow_back_rounded,
            color: CelestialColors.textSecondary.withValues(alpha: 0.78),
          ),
        ),
      ),
    );
  }

  Widget _buildHueLayout() {
    return Column(
      children: [
        const SizedBox(height: 24),
        _buildHeroSection(),
        const SizedBox(height: 20),
        _buildTitleAndStatus(),
        const Spacer(),
        _buildConnectButton(),
        const SizedBox(height: 16),
      ],
    );
  }

  // ── Hero: compact beacon with breathing glow + ripples ─────────────────────

  Widget _buildHeroSection() {
    final b = _breathe.value;
    const heroSize = 150.0;
    const iconSize = 60.0;

    return SizedBox(
      width: heroSize,
      height: heroSize,
      child: Stack(
        alignment: Alignment.center,
        children: [
          // Ripple rings (3 staggered)
          for (int i = 0; i < 3; i++) _buildRippleRing(i, heroSize),

          // Ambient horizon glow
          Container(
            width: iconSize + 40 + b * 16,
            height: iconSize + 40 + b * 16,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              gradient: RadialGradient(
                colors: [
                  _primary.withValues(alpha: 0.10 + b * 0.06),
                  _primary.withValues(alpha: 0.02),
                  Colors.transparent,
                ],
                stops: const [0.0, 0.5, 1.0],
              ),
            ),
          ),

          // Central icon
          Container(
            width: iconSize,
            height: iconSize,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              gradient: LinearGradient(
                begin: Alignment.topLeft,
                end: Alignment.bottomRight,
                colors: [
                  _primary.withValues(alpha: 0.18 + b * 0.08),
                  _primaryDeep.withValues(alpha: 0.08 + b * 0.05),
                ],
              ),
              border: Border.all(
                color: _primary.withValues(alpha: 0.25 + b * 0.15),
                width: 1.2,
              ),
              boxShadow: [
                BoxShadow(
                  color: _primary.withValues(alpha: 0.12 + b * 0.12),
                  blurRadius: 24 + b * 12,
                  spreadRadius: b * 2,
                ),
              ],
            ),
            child: Icon(
              _icon,
              color: _primary.withValues(alpha: 0.78 + b * 0.22),
              size: 28,
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildRippleRing(int index, double size) {
    final phase = (_ripple.value + index * 0.33) % 1.0;
    final scale = 0.4 + phase * 0.6;
    final opacity = (1.0 - phase).clamp(0.0, 1.0) * 0.28;

    return Transform.scale(
      scale: scale,
      child: SizedBox(
        width: size,
        height: size,
        child: DecoratedBox(
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            border: Border.all(
              color: _primary.withValues(alpha: opacity),
              width: 1.0,
            ),
          ),
        ),
      ),
    );
  }

  // ── Title + Dynamic Status ─────────────────────────────────────────────────

  Widget _buildTitleAndStatus() {
    final isServer = widget.mode == ConnectHubMode.rhythmServer;

    // When there are no Homes yet, "Choose Your Home" is misleading — there is
    // nothing to choose. Lead with a welcome so the screen reads as setup.
    var title = _title;
    if (isServer && !_isAddingDevice) {
      final homes = _visibleHomeEntries(context.watch<HomeProvider>());
      if (homes.isEmpty && !_isLoadingAccountHomes) {
        title = 'Welcome to Rhythm';
      }
    }

    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 32),
      child: Column(
        children: [
          Text(
            title,
            textAlign: TextAlign.center,
            style: TextStyle(
              color: CelestialColors.textPrimary,
              fontSize: 22,
              fontWeight: FontWeight.w600,
              letterSpacing: 0.3,
              height: 1.1,
            ),
          ),
          const SizedBox(height: 8),
          if (isServer)
            _buildScanStatusLine()
          else
            Text(
              _subtitle,
              textAlign: TextAlign.center,
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.75),
                fontSize: 14,
                height: 1.45,
                letterSpacing: 0.1,
              ),
            ),
        ],
      ),
    );
  }

  Widget _buildScanStatusLine() {
    final homeProvider = context.watch<HomeProvider>();
    final homes = _visibleHomeEntries(homeProvider);
    final homeCount = homes.length;
    if (!_isAddingDevice) {
      final b = _breathe.value;
      final label = _isLoadingAccountHomes && homeCount == 0
          ? 'Loading your Homes\u2026'
          : homeCount > 0
              ? '$homeCount Home${homeCount == 1 ? '' : 's'}'
              : _accountHomesError ?? 'No Homes yet';
      return _buildStatusLine(
        label: label,
        isLive: _isLoadingAccountHomes,
        active: homeCount > 0,
        breathe: b,
      );
    }

    final existingCount = _visibleDiscoveredDevices(homes).length;
    final newCount = _bleDevices.length;
    final count = existingCount + newCount;
    final b = _breathe.value;

    String label;
    if (count > 0) {
      final parts = <String>[
        if (existingCount > 0)
          '$existingCount existing'
        else if (newCount == 0)
          '0 existing',
        if (newCount > 0) '$newCount new',
      ];
      label = 'Found ${parts.join(', ')}';
    } else if ((_isScanning || _isBleScanning) && count == 0) {
      label = 'Searching nearby hardware\u2026';
    } else {
      label = _bleScanError ?? 'No unassigned devices yet';
    }

    final isLive = _isScanning || _isBleScanning || count > 0;

    return _buildStatusLine(
      label: label,
      isLive: isLive,
      active: count > 0,
      breathe: b,
    );
  }

  Widget _buildStatusLine({
    required String label,
    required bool isLive,
    required bool active,
    required double breathe,
  }) {
    return Row(
      mainAxisAlignment: MainAxisAlignment.center,
      crossAxisAlignment: CrossAxisAlignment.center,
      children: [
        // Pulsing status dot (breathing teal while scanning, steady when found)
        Container(
          width: 7,
          height: 7,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: (_isScanning || _isBleScanning)
                ? _teal.withValues(alpha: 0.35 + breathe * 0.55)
                : (active
                    ? _teal.withValues(alpha: 0.85)
                    : Colors.white.withValues(alpha: 0.22)),
            boxShadow: isLive
                ? [
                    BoxShadow(
                      color: _teal.withValues(alpha: 0.35 + breathe * 0.35),
                      blurRadius: 8 + breathe * 5,
                    ),
                  ]
                : null,
          ),
        ),
        const SizedBox(width: 9),
        Flexible(
          child: Text(
            label,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.85),
              fontSize: 13.5,
              letterSpacing: 0.2,
              fontWeight: FontWeight.w500,
            ),
          ),
        ),
      ],
    );
  }

  // ── Scan button (prominent primary action) ─────────────────────────────────

  Widget _buildScanButton() {
    final isScanning = _isScanning || _isBleScanning;

    return Center(
      child: GestureDetector(
        onTap: isScanning ? null : _scanForAllDevices,
        behavior: HitTestBehavior.opaque,
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 220),
          curve: Curves.easeOut,
          height: 40,
          padding: const EdgeInsets.symmetric(horizontal: 20),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(22),
            gradient: LinearGradient(
              begin: Alignment.topLeft,
              end: Alignment.bottomRight,
              colors: isScanning
                  ? [
                      _teal.withValues(alpha: 0.10),
                      _tealDeep.withValues(alpha: 0.06),
                    ]
                  : [
                      _teal.withValues(alpha: 0.28),
                      _tealDeep.withValues(alpha: 0.18),
                    ],
            ),
            border: Border.all(
              color: _teal.withValues(alpha: isScanning ? 0.3 : 0.55),
              width: 1.2,
            ),
            boxShadow: isScanning
                ? null
                : [
                    BoxShadow(
                      color: _teal.withValues(alpha: 0.22),
                      blurRadius: 16,
                      offset: const Offset(0, 4),
                      spreadRadius: -2,
                    ),
                  ],
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              if (isScanning)
                SizedBox(
                  width: 15,
                  height: 15,
                  child: CircularProgressIndicator(
                    strokeWidth: 1.8,
                    color: _teal.withValues(alpha: 0.85),
                  ),
                )
              else
                Icon(
                  Icons.radar_rounded,
                  color: _teal,
                  size: 17,
                ),
              const SizedBox(width: 9),
              Text(
                isScanning ? 'Scanning\u2026' : 'Scan Again',
                style: TextStyle(
                  color: isScanning
                      ? _teal.withValues(alpha: 0.8)
                      : CelestialColors.textPrimary,
                  fontSize: 14,
                  fontWeight: FontWeight.w600,
                  letterSpacing: 0.4,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }

  // ── Middle: devices list or empty state ────────────────────────────────────

  Widget _buildMiddleContent() {
    final homeProvider = context.watch<HomeProvider>();
    final homes = _visibleHomeEntries(homeProvider);
    final recents = RecentServersService.instance.servers;
    if (_isAddingDevice) {
      return _buildAddDeviceContent(
        homes: homes,
        recents: recents,
      );
    }

    return _buildHomeChooserContent(homes);
  }

  bool get _shouldShowAccountHomeState =>
      widget.mode == ConnectHubMode.rhythmServer &&
      (_isLoadingAccountHomes ||
          _accountHomesError != null ||
          (!_canUseAccountHomes && FeatureFlags.auxSignIn));

  List<AccountHomeServerHubs> _localHomeEntries(HomeProvider homeProvider) {
    return homeProvider.homeServerHubSnapshots;
  }

  List<AccountHomeServerHubs> _visibleCloudHomeEntries(
    HomeProvider homeProvider,
  ) {
    final localIds = homeProvider.homes.map((home) => home.id).toSet();
    return _accountHomes
        .where(
          (snapshot) =>
              snapshot.hasServerHub && !localIds.contains(snapshot.home.id),
        )
        .toList(growable: false);
  }

  List<AccountHomeServerHubs> _visibleHomeEntries(HomeProvider homeProvider) {
    return _mergeHomeEntries(
      localHomes: _localHomeEntries(homeProvider),
      cloudHomes: _visibleCloudHomeEntries(homeProvider),
    );
  }

  Widget _buildHomeChooserContent(List<AccountHomeServerHubs> homes) {
    final showHomeSection = homes.isNotEmpty ||
        _homeActionError != null ||
        _shouldShowAccountHomeState;

    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 24),
      child: ListView(
        physics: const BouncingScrollPhysics(),
        padding: EdgeInsets.zero,
        children: [
          const SizedBox(height: 4),
          if (showHomeSection) ...[
            // ── Path A: enter an existing Home (solid cards) ──────────────
            _buildDiscoverySectionHeader('YOUR HOMES', 'Tap to enter'),
            const SizedBox(height: 4),
            if (_homeActionError != null) ...[
              _buildInlineError(_homeActionError!),
              const SizedBox(height: 12),
            ],
            for (final snapshot in homes) ...[
              _buildHomeCard(snapshot),
              const SizedBox(height: 12),
            ],
            if (_shouldShowAccountHomeState) ...[
              _buildAccountHomeStateCard(),
              const SizedBox(height: 12),
            ],
            // ── Path B: create something new (dashed CTA) ─────────────────
            const SizedBox(height: 20),
            _buildDiscoverySectionHeader('ADD HARDWARE'),
            const SizedBox(height: 4),
          ] else ...[
            _buildNoHomesIntro(),
          ],
          _buildAddDeviceCard(),
        ],
      ),
    );
  }

  /// Friendly first-run lead-in shown when no Homes exist yet, so the single
  /// available action ("Add a Device") reads as a deliberate first step rather
  /// than a lonely, ambiguous card.
  Widget _buildNoHomesIntro() {
    return Padding(
      padding: const EdgeInsets.only(bottom: 16),
      child: Column(
        children: [
          Text(
            "Let's set up your first Home",
            textAlign: TextAlign.center,
            style: TextStyle(
              color: CelestialColors.textPrimary.withValues(alpha: 0.92),
              fontSize: 15.5,
              fontWeight: FontWeight.w600,
              letterSpacing: 0.2,
            ),
          ),
          const SizedBox(height: 7),
          Text(
            'Add a Rhythm Box to create your first Home and\nstart adaptive lighting.',
            textAlign: TextAlign.center,
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.62),
              fontSize: 12.5,
              height: 1.45,
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildAddDeviceContent({
    required List<AccountHomeServerHubs> homes,
    required List<RecentServer> recents,
  }) {
    final visibleRecents = _visibleRecentServers(
      homes: homes,
      recents: recents,
    );
    final recentIds = visibleRecents.map((s) => s.id).toSet();
    // Hide any mDNS hit that's already represented in the recent list — the
    // recent card carries the online dot from the same probe.
    final freshlyDiscovered = _visibleDiscoveredDevices(homes)
        .where((hub) => !recentIds.contains('${hub.address}:${hub.port}'))
        .toList(growable: false);
    final hasHardware = visibleRecents.isNotEmpty ||
        freshlyDiscovered.isNotEmpty ||
        _bleDevices.isNotEmpty;

    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 24),
      child: ListView(
        physics: const BouncingScrollPhysics(),
        padding: EdgeInsets.zero,
        children: [
          if (!hasHardware)
            _buildInlineMutedMessage(
              _isScanning || _isBleScanning
                  ? 'Keep your Box powered on and nearby'
                  : 'No unassigned devices found',
            ),
          if (visibleRecents.isNotEmpty) ...[
            _buildDiscoverySectionHeader(
              'SAVED HARDWARE',
              'Saved on this device',
            ),
            for (final server in visibleRecents) ...[
              _buildRecentDeviceCard(server),
              const SizedBox(height: 8),
            ],
          ],
          if (freshlyDiscovered.isNotEmpty) ...[
            if (visibleRecents.isNotEmpty) const SizedBox(height: 6),
            _buildDiscoverySectionHeader(
              'NEARBY HARDWARE',
              'Found by mDNS',
            ),
            for (final hub in freshlyDiscovered) ...[
              _buildExistingDeviceCard(hub),
              const SizedBox(height: 8),
            ],
          ],
          if (_bleDevices.isNotEmpty) ...[
            if (visibleRecents.isNotEmpty || freshlyDiscovered.isNotEmpty)
              const SizedBox(height: 6),
            _buildDiscoverySectionHeader(
              'NEW HARDWARE',
              'Ready for Bluetooth setup',
            ),
            for (final device in _bleDevices) ...[
              _buildNewDeviceCard(device),
              const SizedBox(height: 8),
            ],
          ],
        ],
      ),
    );
  }

  List<RecentServer> _visibleRecentServers({
    required Iterable<AccountHomeServerHubs> homes,
    required Iterable<RecentServer> recents,
  }) {
    return recents
        .where((server) => !_recentServerIsRepresentedByHome(server, homes))
        .toList(growable: false);
  }

  List<DiscoveredHub> _visibleDiscoveredDevices(
    Iterable<AccountHomeServerHubs> homes,
  ) {
    return _discoveredDevices
        .where((hub) => !_discoveredServerIsRepresentedByHome(hub, homes))
        .toList(growable: false);
  }

  Widget _buildDiscoverySectionHeader(String title, [String? subtitle]) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 8, top: 2),
      child: Row(
        children: [
          Text(
            title,
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.58),
              fontSize: 10,
              fontWeight: FontWeight.w700,
              letterSpacing: 1.25,
            ),
          ),
          const SizedBox(width: 8),
          Expanded(
            child: Container(
              height: 0.5,
              color: CelestialColors.textSecondary.withValues(alpha: 0.12),
            ),
          ),
          if (subtitle != null) ...[
            const SizedBox(width: 8),
            Text(
              subtitle,
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.42),
                fontSize: 10.5,
                fontWeight: FontWeight.w500,
              ),
            ),
          ],
        ],
      ),
    );
  }

  Widget _buildInlineError(String message) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 9),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(10),
        color: Colors.red.withValues(alpha: 0.08),
        border: Border.all(color: Colors.red.withValues(alpha: 0.22)),
      ),
      child: Row(
        children: [
          Icon(
            Icons.error_outline_rounded,
            color: Colors.red.withValues(alpha: 0.78),
            size: 16,
          ),
          const SizedBox(width: 8),
          Expanded(
            child: Text(
              message,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                color: Colors.red.withValues(alpha: 0.82),
                fontSize: 12.5,
                fontWeight: FontWeight.w500,
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildInlineMutedMessage(String message) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(10),
        color: Colors.white.withValues(alpha: 0.035),
        border: Border.all(color: Colors.white.withValues(alpha: 0.08)),
      ),
      child: Row(
        children: [
          Icon(
            Icons.info_outline_rounded,
            color: CelestialColors.textSecondary.withValues(alpha: 0.55),
            size: 16,
          ),
          const SizedBox(width: 8),
          Expanded(
            child: Text(
              message,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.62),
                fontSize: 12.5,
                fontWeight: FontWeight.w500,
              ),
            ),
          ),
        ],
      ),
    );
  }

  /// The "create new" action. Deliberately drawn in a different visual idiom
  /// from the solid Home cards — a glowing dashed-outline CTA — so adding
  /// hardware never reads as just another Home to enter.
  Widget _buildAddDeviceCard() {
    final b = _breathe.value;
    return GestureDetector(
      onTap: _openAddDeviceScreen,
      behavior: HitTestBehavior.opaque,
      child: CustomPaint(
        painter: _DashedRRectPainter(
          color: _teal.withValues(alpha: 0.28 + b * 0.10),
          radius: 14,
          dashWidth: 6,
          dashGap: 5,
          strokeWidth: 1.2,
        ),
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 13),
          child: Row(
            children: [
              // Outlined "+" — a lighter, secondary affordance that signals
              // "create new" without competing with the Home cards above.
              Container(
                width: 36,
                height: 36,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: _teal.withValues(alpha: 0.12),
                  border: Border.all(
                    color: _teal.withValues(alpha: 0.30),
                    width: 1,
                  ),
                ),
                child: Icon(
                  Icons.add_rounded,
                  color: _teal.withValues(alpha: 0.90),
                  size: 20,
                ),
              ),
              const SizedBox(width: 13),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(
                      'Add a Device',
                      style: TextStyle(
                        color:
                            CelestialColors.textPrimary.withValues(alpha: 0.90),
                        fontSize: 14,
                        fontWeight: FontWeight.w600,
                        letterSpacing: 0.2,
                      ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      'Find a Rhythm Box nearby and set up a Home',
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        color: CelestialColors.textSecondary
                            .withValues(alpha: 0.55),
                        fontSize: 11.5,
                        height: 1.3,
                      ),
                    ),
                  ],
                ),
              ),
              const SizedBox(width: 8),
              Icon(
                Icons.arrow_forward_rounded,
                color: _teal.withValues(alpha: 0.60),
                size: 18,
              ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _buildHomeCard(AccountHomeServerHubs snapshot) {
    final currentHomeId = context.watch<HomeProvider>().currentHome?.id;
    final isCurrent = currentHomeId == snapshot.home.id;
    final isBusy = _enteringHomeId == snapshot.home.id ||
        _deletingHomeId == snapshot.home.id;
    final serverHub = snapshot.preferredServerHub;
    final hasServer = serverHub != null;
    final color = hasServer ? _teal : CelestialColors.textSecondary;

    final subtitle = _homeSubtitle(
      snapshot: snapshot,
      isCurrent: isCurrent,
      serverHub: serverHub,
    );
    final badge = isCurrent ? 'Current' : 'Home';

    return GestureDetector(
      onTap: isBusy ? null : () => _enterHome(snapshot),
      onLongPress: isBusy ? null : () => _confirmDeleteHome(snapshot),
      behavior: HitTestBehavior.opaque,
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 200),
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 16),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(16),
          // The current Home reads as the headline: gradient fill, brighter
          // border and a soft outer glow lift it clearly above its siblings.
          gradient: isCurrent
              ? LinearGradient(
                  begin: Alignment.topLeft,
                  end: Alignment.bottomRight,
                  colors: [
                    color.withValues(alpha: 0.20),
                    color.withValues(alpha: 0.07),
                  ],
                )
              : null,
          color: isCurrent ? null : Colors.white.withValues(alpha: 0.05),
          border: Border.all(
            color: isCurrent
                ? color.withValues(alpha: 0.55)
                : Colors.white.withValues(alpha: 0.10),
            width: isCurrent ? 1.5 : 1,
          ),
          boxShadow: isCurrent
              ? [
                  BoxShadow(
                    color: color.withValues(alpha: 0.22),
                    blurRadius: 22,
                    spreadRadius: -4,
                    offset: const Offset(0, 6),
                  ),
                ]
              : null,
        ),
        child: Row(
          children: [
            Container(
              width: 46,
              height: 46,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: color.withValues(
                  alpha: hasServer ? (isCurrent ? 0.24 : 0.14) : 0.08,
                ),
                border: isCurrent
                    ? Border.all(
                        color: color.withValues(alpha: 0.40),
                        width: 1,
                      )
                    : null,
              ),
              child: Icon(
                hasServer ? Icons.home_rounded : Icons.home_outlined,
                color: color.withValues(alpha: hasServer ? 0.95 : 0.65),
                size: 24,
              ),
            ),
            const SizedBox(width: 14),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisSize: MainAxisSize.min,
                children: [
                  Row(
                    children: [
                      Flexible(
                        child: Text(
                          snapshot.home.name,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                            color: CelestialColors.textPrimary,
                            fontSize: 17,
                            fontWeight:
                                isCurrent ? FontWeight.w700 : FontWeight.w600,
                            letterSpacing: 0.1,
                          ),
                        ),
                      ),
                      const SizedBox(width: 8),
                      _buildDiscoveryBadge(badge, color),
                    ],
                  ),
                  const SizedBox(height: 4),
                  Text(
                    subtitle,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color:
                          CelestialColors.textSecondary.withValues(alpha: 0.62),
                      fontSize: 12.5,
                      letterSpacing: 0.2,
                    ),
                  ),
                ],
              ),
            ),
            const SizedBox(width: 10),
            if (isBusy)
              SizedBox(
                width: 18,
                height: 18,
                child: CircularProgressIndicator(
                  strokeWidth: 1.8,
                  color: color.withValues(alpha: 0.8),
                ),
              )
            else
              Container(
                width: 30,
                height: 30,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: isCurrent
                      ? color.withValues(alpha: 0.20)
                      : Colors.white.withValues(alpha: 0.05),
                ),
                child: Icon(
                  Icons.arrow_forward_rounded,
                  color: color.withValues(alpha: isCurrent ? 0.95 : 0.70),
                  size: 18,
                ),
              ),
          ],
        ),
      ),
    );
  }

  String _homeSubtitle({
    required AccountHomeServerHubs snapshot,
    required bool isCurrent,
    required Hub? serverHub,
  }) {
    final prefix = isCurrent ? 'Selected • ' : '';
    if (serverHub == null) {
      return '${prefix}No Box saved yet';
    }

    final remote = serverHub.remoteEndpoint;
    final count = snapshot.serverHubs.length;
    final suffix = count == 1 ? '1 Box' : '$count Boxes';
    if (remote != null) {
      return '$prefix$suffix • connects automatically';
    }
    return '$prefix$suffix saved';
  }

  Future<void> _confirmDeleteHome(AccountHomeServerHubs snapshot) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: Text(
          'Delete ${snapshot.home.name}?',
          style: const TextStyle(color: CelestialColors.textPrimary),
        ),
        content: const Text(
          'This removes the Home and its saved Boxes from this app. If it is saved to your account, it will be removed there too.',
          style: TextStyle(color: CelestialColors.textSecondary),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: Text(
              'Cancel',
              style: TextStyle(color: CelestialColors.textSecondary),
            ),
          ),
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(true),
            child: const Text(
              'Delete',
              style: TextStyle(color: Colors.redAccent),
            ),
          ),
        ],
      ),
    );
    if (confirmed != true || !mounted) return;

    final homeProvider = context.read<HomeProvider>();
    final localHomeIds = homeProvider.homes.map((home) => home.id).toSet();
    final representedHomeIds = _homeIdsRepresentedBy(
      snapshot: snapshot,
      localHomes: _localHomeEntries(homeProvider),
      cloudHomes: _accountHomes,
    );

    setState(() {
      _deletingHomeId = snapshot.home.id;
      _homeActionError = null;
    });

    var success = true;
    for (final homeId in representedHomeIds) {
      if (localHomeIds.contains(homeId)) {
        success = await homeProvider.deleteHome(homeId) && success;
      }
      await AccountCloudSyncService.instance.deleteHome(
        homeId: homeId,
        reason: 'home_deleted',
      );
    }

    if (!mounted) return;
    setState(() {
      _deletingHomeId = null;
      _accountHomes = _accountHomes
          .where((snapshot) => !representedHomeIds.contains(snapshot.home.id))
          .toList(growable: false);
      _homeActionError =
          success ? null : 'Could not delete ${snapshot.home.name}';
    });

    if (success && _canUseAccountHomes) {
      unawaited(_refreshAccountHomes());
    }
  }

  Widget _buildAccountHomeStateCard() {
    if (_isLoadingAccountHomes) {
      return _buildHomeActionCard(
        icon: Icons.sync_rounded,
        title: 'Loading Homes',
        subtitle: 'Checking your signed-in account',
        busy: true,
        onTap: null,
      );
    }

    if (_accountHomesError != null) {
      return _buildHomeActionCard(
        icon: Icons.cloud_off_rounded,
        title: _accountHomesError!,
        subtitle: 'Tap to retry',
        color: Colors.redAccent,
        onTap: _refreshAccountHomes,
      );
    }

    if (!_canUseAccountHomes && FeatureFlags.auxSignIn) {
      return _buildHomeActionCard(
        icon: Icons.account_circle_outlined,
        title: 'Sign in for saved Homes',
        subtitle: 'Use your account to enter a Home away from local hardware',
        color: const Color(0xFF8AB4F8),
        onTap: () => SignInModal.show(context),
      );
    }

    return const SizedBox.shrink();
  }

  Widget _buildHomeActionCard({
    required IconData icon,
    required String title,
    required String subtitle,
    required VoidCallback? onTap,
    Color? color,
    bool busy = false,
  }) {
    final resolvedColor = color ?? _teal;
    return GestureDetector(
      onTap: busy ? null : onTap,
      behavior: HitTestBehavior.opaque,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(12),
          color: resolvedColor.withValues(alpha: 0.07),
          border: Border.all(color: resolvedColor.withValues(alpha: 0.18)),
        ),
        child: Row(
          children: [
            SizedBox(
              width: 34,
              height: 34,
              child: Center(
                child: busy
                    ? CircularProgressIndicator(
                        strokeWidth: 1.7,
                        color: resolvedColor.withValues(alpha: 0.8),
                      )
                    : Icon(
                        icon,
                        color: resolvedColor.withValues(alpha: 0.86),
                        size: 19,
                      ),
              ),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisSize: MainAxisSize.min,
                children: [
                  Text(
                    title,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 14,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    subtitle,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color:
                          CelestialColors.textSecondary.withValues(alpha: 0.58),
                      fontSize: 11.5,
                    ),
                  ),
                ],
              ),
            ),
            if (!busy && onTap != null)
              Icon(
                Icons.arrow_forward_rounded,
                color: resolvedColor.withValues(alpha: 0.7),
                size: 18,
              ),
          ],
        ),
      ),
    );
  }

  Widget _buildRecentDeviceCard(RecentServer server) {
    final isOnline = RecentServersService.instance.isOnline(server.id);
    final endpointKey = _rhythmServerEndpointKey(server.host, server.port);
    final hasError = _connectError == endpointKey;
    final isBusy = _connectingEndpoint == endpointKey;
    final b = _breathe.value;

    // Reuse the connect path: the recent entry behaves exactly like a
    // discovered hub, just sourced from local storage instead of mDNS.
    final hub = DiscoveredHub(
      host: server.host,
      port: server.port,
      address: server.host,
      name: server.name,
      type: HubType.server,
    );

    final Color pipColor;
    if (hasError) {
      pipColor = Colors.red.withValues(alpha: 0.85);
    } else if (isOnline) {
      pipColor = Color.lerp(
        _teal.withValues(alpha: 0.45),
        _teal,
        b,
      )!;
    } else {
      pipColor = CelestialColors.textSecondary.withValues(alpha: 0.35);
    }

    final String subtitle;
    if (hasError) {
      subtitle = _connectErrorMessage ?? 'tap to retry';
    } else if (isOnline) {
      subtitle = '${server.host} • Online';
    } else if (_isScanning) {
      subtitle = '${server.host} • Checking…';
    } else {
      subtitle = '${server.host} • Offline';
    }

    return GestureDetector(
      onTap: _isConnecting ? null : () => _connectToDevice(hub),
      onLongPress: () => _confirmForgetRecent(server),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(12),
          color: Colors.white.withValues(alpha: 0.04),
          border: Border.all(
            color: hasError
                ? Colors.red.withValues(alpha: 0.35)
                : isOnline
                    ? _teal.withValues(alpha: 0.28)
                    : Colors.white.withValues(alpha: 0.10),
            width: 1,
          ),
        ),
        child: Row(
          children: [
            SizedBox(
              width: 34,
              height: 34,
              child: Stack(
                children: [
                  Container(
                    width: 34,
                    height: 34,
                    decoration: BoxDecoration(
                      shape: BoxShape.circle,
                      color: _teal.withValues(alpha: isOnline ? 0.14 : 0.08),
                    ),
                    child: Icon(
                      Icons.developer_board_rounded,
                      color: _teal.withValues(alpha: isOnline ? 0.92 : 0.55),
                      size: 17,
                    ),
                  ),
                  Positioned(
                    top: 1,
                    right: 1,
                    child: Container(
                      width: 7,
                      height: 7,
                      decoration: BoxDecoration(
                        shape: BoxShape.circle,
                        color: pipColor,
                        border: Border.all(
                          color: CelestialColors.backgroundDark
                              .withValues(alpha: 0.9),
                          width: 1.2,
                        ),
                      ),
                    ),
                  ),
                ],
              ),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisSize: MainAxisSize.min,
                children: [
                  Row(
                    children: [
                      Expanded(
                        child: Text(
                          server.name,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                            color: CelestialColors.textPrimary.withValues(
                              alpha: isOnline ? 1.0 : 0.75,
                            ),
                            fontSize: 14,
                            fontWeight: FontWeight.w500,
                            letterSpacing: 0.1,
                          ),
                        ),
                      ),
                      _buildDiscoveryBadge('Recent', _teal),
                    ],
                  ),
                  const SizedBox(height: 2),
                  Text(
                    subtitle,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: hasError
                          ? Colors.red.withValues(alpha: 0.75)
                          : CelestialColors.textSecondary
                              .withValues(alpha: 0.55),
                      fontSize: 11.5,
                      fontFamily: hasError ? null : 'monospace',
                      letterSpacing: 0.3,
                    ),
                  ),
                ],
              ),
            ),
            if (isBusy)
              SizedBox(
                width: 16,
                height: 16,
                child: CircularProgressIndicator(
                  strokeWidth: 1.6,
                  color: _teal.withValues(alpha: 0.7),
                ),
              )
            else
              Icon(
                Icons.arrow_forward_rounded,
                color: _teal.withValues(alpha: isOnline ? 0.7 : 0.4),
                size: 18,
              ),
          ],
        ),
      ),
    );
  }

  Future<void> _confirmForgetRecent(RecentServer server) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: Text(
          'Forget ${server.name}?',
          style: const TextStyle(color: CelestialColors.textPrimary),
        ),
        content: Text(
          'Removes ${server.host} from your recent list. You can re-add it any time by connecting again.',
          style: const TextStyle(color: CelestialColors.textSecondary),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(false),
            child: Text(
              'Cancel',
              style: TextStyle(color: CelestialColors.textSecondary),
            ),
          ),
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(true),
            child: const Text(
              'Forget',
              style: TextStyle(color: Colors.redAccent),
            ),
          ),
        ],
      ),
    );
    if (confirmed == true) {
      await RecentServersService.instance.remove(server.id);
    }
  }

  Widget _buildExistingDeviceCard(DiscoveredHub hub) {
    final endpointKey = _discoveredEndpointKey(hub);
    final hasError = _connectError == endpointKey;
    final isBusy = _connectingEndpoint == endpointKey;
    final b = _breathe.value;

    return GestureDetector(
      onTap: _isConnecting ? null : () => _connectToDevice(hub),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(12),
          color: Colors.white.withValues(alpha: 0.04),
          border: Border.all(
            color: hasError
                ? Colors.red.withValues(alpha: 0.35)
                : _teal.withValues(alpha: 0.22),
            width: 1,
          ),
        ),
        child: Row(
          children: [
            // Icon puck with a tiny live pip in the top-right corner
            SizedBox(
              width: 34,
              height: 34,
              child: Stack(
                children: [
                  Container(
                    width: 34,
                    height: 34,
                    decoration: BoxDecoration(
                      shape: BoxShape.circle,
                      color: _teal.withValues(alpha: 0.12),
                    ),
                    child: Icon(
                      Icons.developer_board_rounded,
                      color: _teal.withValues(alpha: 0.9),
                      size: 17,
                    ),
                  ),
                  Positioned(
                    top: 1,
                    right: 1,
                    child: Container(
                      width: 7,
                      height: 7,
                      decoration: BoxDecoration(
                        shape: BoxShape.circle,
                        color: hasError
                            ? Colors.red.withValues(alpha: 0.85)
                            : Color.lerp(
                                _teal.withValues(alpha: 0.45),
                                _teal,
                                b,
                              ),
                        border: Border.all(
                          color: CelestialColors.backgroundDark
                              .withValues(alpha: 0.9),
                          width: 1.2,
                        ),
                      ),
                    ),
                  ),
                ],
              ),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisSize: MainAxisSize.min,
                children: [
                  Row(
                    children: [
                      Expanded(
                        child: Text(
                          hub.name ?? 'RhythmServer',
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                            color: CelestialColors.textPrimary,
                            fontSize: 14,
                            fontWeight: FontWeight.w500,
                            letterSpacing: 0.1,
                          ),
                        ),
                      ),
                      _buildDiscoveryBadge('Existing', _teal),
                    ],
                  ),
                  const SizedBox(height: 2),
                  Text(
                    hasError
                        ? (_connectErrorMessage ?? 'tap to retry')
                        : hub.address,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: hasError
                          ? Colors.red.withValues(alpha: 0.75)
                          : CelestialColors.textSecondary
                              .withValues(alpha: 0.55),
                      fontSize: 11.5,
                      fontFamily: hasError ? null : 'monospace',
                      letterSpacing: 0.3,
                    ),
                  ),
                ],
              ),
            ),
            if (isBusy)
              SizedBox(
                width: 16,
                height: 16,
                child: CircularProgressIndicator(
                  strokeWidth: 1.6,
                  color: _teal.withValues(alpha: 0.7),
                ),
              )
            else
              Icon(
                Icons.arrow_forward_rounded,
                color: _teal.withValues(alpha: 0.6),
                size: 18,
              ),
          ],
        ),
      ),
    );
  }

  Widget _buildNewDeviceCard(BleDevice device) {
    return GestureDetector(
      onTap: _isConnecting ? null : () => _openBleProvisioning(device),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(12),
          color: Colors.white.withValues(alpha: 0.04),
          border: Border.all(
            color: CelestialColors.sunWarm.withValues(alpha: 0.25),
            width: 1,
          ),
        ),
        child: Row(
          children: [
            Container(
              width: 34,
              height: 34,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: CelestialColors.sunWarm.withValues(alpha: 0.14),
              ),
              child: Icon(
                Icons.bluetooth_searching_rounded,
                color: CelestialColors.sunWarm.withValues(alpha: 0.92),
                size: 17,
              ),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisSize: MainAxisSize.min,
                children: [
                  Row(
                    children: [
                      Expanded(
                        child: Text(
                          device.name,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                            color: CelestialColors.textPrimary,
                            fontSize: 14,
                            fontWeight: FontWeight.w500,
                            letterSpacing: 0.1,
                          ),
                        ),
                      ),
                      _buildDiscoveryBadge('New', CelestialColors.sunWarm),
                    ],
                  ),
                  const SizedBox(height: 2),
                  Text(
                    'Bluetooth setup • signal ${_signalLabel(device.rssi)}',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color:
                          CelestialColors.textSecondary.withValues(alpha: 0.55),
                      fontSize: 11.5,
                      letterSpacing: 0.2,
                    ),
                  ),
                ],
              ),
            ),
            Icon(
              Icons.arrow_forward_rounded,
              color: CelestialColors.sunWarm.withValues(alpha: 0.7),
              size: 18,
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildDiscoveryBadge(String label, Color color) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 2),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(8),
        color: color.withValues(alpha: 0.14),
        border: Border.all(color: color.withValues(alpha: 0.22)),
      ),
      child: Text(
        label,
        style: TextStyle(
          color: color.withValues(alpha: 0.95),
          fontSize: 9.5,
          fontWeight: FontWeight.w700,
          letterSpacing: 0.35,
        ),
      ),
    );
  }

  String _signalLabel(int rssi) {
    if (rssi >= -55) return 'strong';
    if (rssi >= -70) return 'good';
    if (rssi >= -82) return 'fair';
    return 'weak';
  }

  // ── Bottom: compact secondary actions ──────────────────────────────────────

  Widget _buildBottomActions(bool keyboardOpen) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 24),
      child: Column(
        children: [
          if (LocalRhythmServerService.instance.canManageLocalServer) ...[
            _buildLocalServerButton(),
            const SizedBox(height: 10),
          ],
          // Manual IP sits with the "find an existing box" family
          _buildCompactManualIp(),
        ],
      ),
    );
  }

  Widget _buildLocalServerButton() {
    final isBusy = _isStartingLocalServer;
    final hasError = _localServerError != null;

    return GestureDetector(
      onTap: isBusy ? null : _startLocalRhythmServer,
      behavior: HitTestBehavior.opaque,
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 180),
        height: 46,
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(12),
          color: _teal.withValues(alpha: isBusy ? 0.08 : 0.12),
          border: Border.all(
            color: hasError
                ? Colors.red.withValues(alpha: 0.36)
                : _teal.withValues(alpha: isBusy ? 0.22 : 0.34),
          ),
        ),
        padding: const EdgeInsets.symmetric(horizontal: 12),
        child: Row(
          children: [
            SizedBox(
              width: 18,
              height: 18,
              child: isBusy
                  ? CircularProgressIndicator(
                      strokeWidth: 1.8,
                      color: _teal.withValues(alpha: 0.8),
                    )
                  : Icon(
                      Icons.computer_rounded,
                      color: _teal.withValues(alpha: 0.9),
                      size: 18,
                    ),
            ),
            const SizedBox(width: 10),
            Expanded(
              child: Text(
                hasError
                    ? _localServerError!
                    : isBusy
                        ? 'Starting Local Rhythm Server...'
                        : 'Start Local Rhythm Server',
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  color: hasError
                      ? Colors.red.withValues(alpha: 0.82)
                      : CelestialColors.textPrimary,
                  fontSize: 14,
                  fontWeight: FontWeight.w600,
                  letterSpacing: 0.1,
                ),
              ),
            ),
            Icon(
              Icons.arrow_forward_rounded,
              color: _teal.withValues(alpha: isBusy ? 0.35 : 0.85),
              size: 18,
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildCompactManualIp() {
    final hasError = _manualConnectError != null;
    final isBusy = _isManualConnecting;

    return Container(
      height: 46,
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(12),
        color: Colors.white.withValues(alpha: 0.04),
        border: Border.all(
          color: hasError
              ? Colors.red.withValues(alpha: 0.4)
              : _manualIpFocus.hasFocus
                  ? _teal.withValues(alpha: 0.55)
                  : Colors.white.withValues(alpha: 0.10),
          width: 1,
        ),
      ),
      child: Row(
        children: [
          const SizedBox(width: 12),
          Icon(
            Icons.dns_rounded,
            color: _manualIpFocus.hasFocus
                ? _teal.withValues(alpha: 0.9)
                : CelestialColors.textSecondary.withValues(alpha: 0.55),
            size: 16,
          ),
          const SizedBox(width: 10),
          Expanded(
            child: TextField(
              controller: _manualIpController,
              focusNode: _manualIpFocus,
              enabled: !isBusy,
              keyboardType: TextInputType.url,
              textInputAction: TextInputAction.go,
              autocorrect: false,
              enableSuggestions: false,
              style: TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 14,
                fontFamily: 'monospace',
                letterSpacing: 0.4,
              ),
              decoration: InputDecoration(
                isDense: true,
                hintText: hasError
                    ? _manualConnectError
                    : 'Connect by IP (e.g. 192.168.1.100)',
                hintStyle: TextStyle(
                  color: hasError
                      ? Colors.red.withValues(alpha: 0.7)
                      : CelestialColors.textSecondary.withValues(alpha: 0.42),
                  fontSize: 12.5,
                  fontFamily: hasError ? null : 'monospace',
                ),
                border: InputBorder.none,
                contentPadding: EdgeInsets.zero,
              ),
              onChanged: (_) {
                if (_manualConnectError != null) {
                  setState(() => _manualConnectError = null);
                }
              },
              onSubmitted: (_) => _submitManualIp(),
            ),
          ),
          GestureDetector(
            onTap: isBusy ? null : _submitManualIp,
            behavior: HitTestBehavior.opaque,
            child: Container(
              width: 44,
              height: 46,
              alignment: Alignment.center,
              child: isBusy
                  ? SizedBox(
                      width: 14,
                      height: 14,
                      child: CircularProgressIndicator(
                        strokeWidth: 1.6,
                        color: _teal.withValues(alpha: 0.75),
                      ),
                    )
                  : Icon(
                      Icons.arrow_forward_rounded,
                      color: _teal.withValues(alpha: 0.85),
                      size: 18,
                    ),
            ),
          ),
        ],
      ),
    );
  }

  // ── Hue-only primary button ────────────────────────────────────────────────

  Widget _buildConnectButton() {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 48),
      child: GestureDetector(
        onTap: _openConfigurator,
        child: Container(
          padding: const EdgeInsets.symmetric(vertical: 16),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(16),
            gradient: LinearGradient(
              begin: Alignment.topLeft,
              end: Alignment.bottomRight,
              colors: [_primary, _primaryDeep],
            ),
            boxShadow: [
              BoxShadow(
                color: _primary.withValues(alpha: 0.35),
                blurRadius: 20,
                offset: const Offset(0, 6),
                spreadRadius: -2,
              ),
              BoxShadow(
                color: _primaryDeep.withValues(alpha: 0.2),
                blurRadius: 40,
                offset: const Offset(0, 12),
                spreadRadius: -4,
              ),
            ],
          ),
          child: Row(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              Icon(
                _icon,
                color: Colors.white,
                size: 20,
              ),
              const SizedBox(width: 10),
              Text(
                _buttonLabel,
                style: const TextStyle(
                  color: Colors.white,
                  fontSize: 16,
                  fontWeight: FontWeight.w600,
                  letterSpacing: 0.3,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _AuthTokenRequiredException implements Exception {
  final String message;

  const _AuthTokenRequiredException(this.message);
}

/// Paints a dashed rounded-rectangle outline. Used to give the "Add a Device"
/// action a distinct "create new" affordance versus the solid Home cards.
class _DashedRRectPainter extends CustomPainter {
  _DashedRRectPainter({
    required this.color,
    required this.radius,
    this.dashWidth = 6,
    this.dashGap = 5,
    this.strokeWidth = 1.4,
  });

  final Color color;
  final double radius;
  final double dashWidth;
  final double dashGap;
  final double strokeWidth;

  @override
  void paint(Canvas canvas, Size size) {
    final path = Path()
      ..addRRect(
        RRect.fromRectAndRadius(
          Offset.zero & size,
          Radius.circular(radius),
        ),
      );
    final paint = Paint()
      ..color = color
      ..style = PaintingStyle.stroke
      ..strokeWidth = strokeWidth;

    for (final metric in path.computeMetrics()) {
      var distance = 0.0;
      while (distance < metric.length) {
        final next = distance + dashWidth;
        canvas.drawPath(
          metric.extractPath(distance, next.clamp(0.0, metric.length)),
          paint,
        );
        distance = next + dashGap;
      }
    }
  }

  @override
  bool shouldRepaint(_DashedRRectPainter old) =>
      old.color != color ||
      old.radius != radius ||
      old.dashWidth != dashWidth ||
      old.dashGap != dashGap ||
      old.strokeWidth != strokeWidth;
}
