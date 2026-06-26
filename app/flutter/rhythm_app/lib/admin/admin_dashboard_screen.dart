import 'dart:async';
import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:http/http.dart' as http;
import 'package:rhythm_core/rhythm_core.dart' show Home, Hub, HubType;
import 'package:rhythm_sdk/rhythm_sdk.dart' show RhythmDeviceType, RhythmHello;
import 'package:shared_preferences/shared_preferences.dart';

import '../backend/backend.dart';
import '../services/account_cloud_sync_service.dart';
import '../services/auth_service.dart';
import 'admin_support_data_service.dart';

class _AdminColors {
  static const background = Color(0xFF0D1117);
  static const surface = Color(0xFF151A22);
  static const surfaceMuted = Color(0xFF111720);
  static const field = Color(0xFF0F141B);
  static const border = Color(0xFF2A3443);
  static const borderMuted = Color(0xFF222B38);
  static const text = Colors.white;
  static const muted = Color(0xFFA1ADBC);
  static const dim = Color(0xFF788493);
  static const accent = Color(0xFF92D7BB);
  static const accentWash = Color(0x2420C997);
  static const warning = Color(0xFFEFC77E);
  static const danger = Color(0xFFFFB4AB);
  static const dangerWash = Color(0x24FFB4AB);
  static const neutralWash = Color(0x2436424F);
}

enum _LightBoxLiveStatusKind {
  checking,
  local,
  remote,
  offline,
}

class _LightBoxLiveStatus {
  const _LightBoxLiveStatus.checking()
      : kind = _LightBoxLiveStatusKind.checking,
        checkedAt = null,
        inventory = null;

  _LightBoxLiveStatus.online(this.kind, {this.inventory})
      : checkedAt = DateTime.now();

  _LightBoxLiveStatus.offline()
      : kind = _LightBoxLiveStatusKind.offline,
        checkedAt = DateTime.now(),
        inventory = null;

  final _LightBoxLiveStatusKind kind;
  final DateTime? checkedAt;
  final _LightBoxInventory? inventory;
}

class _LightBoxInventory {
  const _LightBoxInventory({
    required this.lights,
    required this.buttons,
    required this.motionSensors,
    required this.otherDevices,
  });

  final int lights;
  final int buttons;
  final int motionSensors;
  final int otherDevices;

  int get total => lights + buttons + motionSensors + otherDevices;
}

class _HomeSearchResult {
  const _HomeSearchResult({
    required this.customer,
    required this.snapshot,
  });

  final SupportCustomerHomes customer;
  final AccountHomeServerHubs snapshot;

  Home get home => snapshot.home;
  List<Hub> get lightBoxes => snapshot.serverHubs;
  bool get hasLightBoxes => lightBoxes.isNotEmpty;

  String get customerTitle => customer.customerLabel;
  String? get customerSubtitle => customer.secondaryLabel;
  String get homeLabel => home.name;
  String get locationLabel => _homeLocationLabel(home);

  String get searchText {
    return [
      customer.customerLabel,
      if (customer.customerEmail != null) customer.customerEmail!,
      if (customer.customerName != null) customer.customerName!,
      customer.ownerId,
      home.id,
      home.name,
      if (home.location?.cityName != null) home.location!.cityName!,
      if (home.timezone != null) home.timezone!,
      for (final box in lightBoxes) ...[
        box.id,
        box.name,
        box.endpoint.host,
        if (box.remoteEndpoint != null) box.remoteEndpoint!.host,
        if (box.serverInstanceId != null) box.serverInstanceId!,
      ],
    ].join(' ').toLowerCase();
  }
}

class AdminDashboardScreen extends StatefulWidget {
  const AdminDashboardScreen({super.key});

  @override
  State<AdminDashboardScreen> createState() => _AdminDashboardScreenState();
}

class _AdminDashboardScreenState extends State<AdminDashboardScreen> {
  static const _maxResults = 100;
  static const _maxRecentHomes = 10;
  static const _recentHomesKeyPrefix = 'rhythm_admin_recent_homes';

  late Future<AdminSupportSnapshot> _supportFuture;
  StreamSubscription<AuthUser?>? _authSubscription;

  final TextEditingController _emailController = TextEditingController();
  final TextEditingController _searchController = TextEditingController();
  final Map<String, _LightBoxLiveStatus> _liveStatuses = {};
  List<String> _recentHomeIds = const [];

  int _statusCheckGeneration = 0;
  bool _isAuthWorking = false;
  bool _showEmptyHomes = false;
  String? _authError;
  String? _authNotice;
  String? _selectedHomeId;

  @override
  void initState() {
    super.initState();
    _supportFuture = _loadSupportSnapshot();
    unawaited(_loadRecentHomes());
    _searchController.addListener(_onSearchChanged);
    final currentEmail = AuthService().currentUser?.email;
    if (currentEmail != null) {
      _emailController.text = currentEmail;
    }
    _authSubscription = AuthService().authStateChanges.listen((user) {
      if (!mounted) return;
      setState(() {
        if (user?.email != null) {
          _emailController.text = user!.email!;
        }
        _authError = null;
        _authNotice = null;
        _selectedHomeId = null;
        _supportFuture = _loadSupportSnapshot();
        _liveStatuses.clear();
      });
      unawaited(_loadRecentHomes());
    });
  }

  @override
  void dispose() {
    _authSubscription?.cancel();
    _emailController.dispose();
    _searchController
      ..removeListener(_onSearchChanged)
      ..dispose();
    super.dispose();
  }

  void _onSearchChanged() {
    if (mounted) setState(() {});
  }

  Future<AdminSupportSnapshot> _loadSupportSnapshot() {
    _statusCheckGeneration++;
    return AdminSupportDataService.instance.loadCustomersAndServerHubs();
  }

  String _recentHomesKey() {
    final userId = AuthService().currentUserId?.trim();
    final suffix = userId != null && userId.isNotEmpty ? userId : 'signed-out';
    return '${_recentHomesKeyPrefix}_$suffix';
  }

  Future<void> _loadRecentHomes() async {
    final prefs = await SharedPreferences.getInstance();
    final stored = prefs.getStringList(_recentHomesKey()) ?? const <String>[];
    final seen = <String>{};
    final recent = <String>[
      for (final id in stored)
        if (id.trim().isNotEmpty && seen.add(id.trim())) id.trim(),
    ].take(_maxRecentHomes).toList(growable: false);
    if (!mounted) return;
    setState(() => _recentHomeIds = recent);
  }

  Future<void> _rememberRecentHome(_HomeSearchResult result) async {
    final homeId = result.home.id.trim();
    if (homeId.isEmpty) return;
    if (_recentHomeIds.contains(homeId)) return;
    final updated = <String>[
      homeId,
      for (final id in _recentHomeIds) id,
    ].take(_maxRecentHomes).toList(growable: false);
    setState(() => _recentHomeIds = updated);
    final prefs = await SharedPreferences.getInstance();
    await prefs.setStringList(_recentHomesKey(), updated);
  }

  void _refresh() {
    setState(() {
      _selectedHomeId = null;
      _liveStatuses.clear();
      _supportFuture = _loadSupportSnapshot();
    });
  }

  void _selectHome(_HomeSearchResult result) {
    setState(() {
      _selectedHomeId = result.home.id;
    });
    unawaited(_rememberRecentHome(result));
    unawaited(_checkHomeLightBoxes(result.snapshot));
  }

  void _openLightBox(
    BuildContext context,
    _HomeSearchResult result,
    Hub hub,
    _LightBoxLiveStatus? liveStatus,
  ) {
    unawaited(_rememberRecentHome(result));
    unawaited(_showOpenLightBoxDialog(context, result, hub, liveStatus));
  }

  Future<void> _checkHomeLightBoxes(AccountHomeServerHubs snapshot) async {
    final generation = ++_statusCheckGeneration;
    final hubs = snapshot.serverHubs;
    if (hubs.isEmpty) return;
    setState(() {
      for (final hub in hubs) {
        _liveStatuses[hub.id] = const _LightBoxLiveStatus.checking();
      }
    });
    await Future.wait([
      for (final hub in hubs) _checkLightBox(hub, generation),
    ]);
  }

  Future<void> _checkLightBox(Hub hub, int generation) async {
    final status = await _probeLightBox(hub);
    if (!mounted || generation != _statusCheckGeneration) return;
    setState(() {
      _liveStatuses[hub.id] = status;
    });
  }

  Future<_LightBoxLiveStatus> _probeLightBox(Hub hub) async {
    if (await _healthCheckBaseUrl(hub.endpoint.baseUrl)) {
      final inventory = await _loadLightBoxInventory(
        hub,
        preferredBaseUrl: hub.endpoint.baseUrl,
      );
      return _LightBoxLiveStatus.online(
        _LightBoxLiveStatusKind.local,
        inventory: inventory,
      );
    }
    final remote = hub.remoteEndpoint;
    if (remote != null &&
        remote.baseUrl.toLowerCase() != hub.endpoint.baseUrl.toLowerCase() &&
        await _healthCheckBaseUrl(remote.baseUrl)) {
      final inventory = await _loadLightBoxInventory(
        hub,
        preferredBaseUrl: remote.baseUrl,
      );
      return _LightBoxLiveStatus.online(
        _LightBoxLiveStatusKind.remote,
        inventory: inventory,
      );
    }
    return _LightBoxLiveStatus.offline();
  }

  Future<_LightBoxInventory?> _loadLightBoxInventory(
    Hub hub, {
    required String preferredBaseUrl,
  }) async {
    final candidates = <String>[
      preferredBaseUrl,
      hub.endpoint.baseUrl,
      if (hub.remoteEndpoint != null) hub.remoteEndpoint!.baseUrl,
    ];
    final seen = <String>{};
    for (final baseUrl in candidates) {
      final normalized = baseUrl.toLowerCase();
      if (!seen.add(normalized)) continue;
      final inventory = await _fetchLightBoxInventory(hub, baseUrl);
      if (inventory != null) return inventory;
    }
    return null;
  }

  Future<_LightBoxInventory?> _fetchLightBoxInventory(
    Hub hub,
    String baseUrl,
  ) async {
    try {
      final response = await http
          .get(
            _stateUri(baseUrl),
            headers: _authHeaders(hub.token),
          )
          .timeout(const Duration(seconds: 4));
      if (response.statusCode < 200 || response.statusCode >= 300) {
        return null;
      }
      final decoded = jsonDecode(response.body);
      if (decoded is! Map) return null;
      final state = RhythmHello.fromJson(_stringKeyMap(decoded));
      return _inventoryFromState(state);
    } catch (_) {
      return null;
    }
  }

  Future<bool> _healthCheckBaseUrl(String baseUrl) async {
    try {
      final response = await http
          .get(_healthCheckUri(baseUrl))
          .timeout(const Duration(seconds: 3));
      if (response.statusCode < 200 || response.statusCode >= 300) {
        return false;
      }
      final decoded = jsonDecode(response.body);
      return decoded is Map && decoded['status'] == 'healthy';
    } catch (_) {
      return false;
    }
  }

  Uri _healthCheckUri(String baseUrl) =>
      _uriWithAppendedPath(baseUrl, 'health');

  Uri _stateUri(String baseUrl) => _uriWithAppendedPath(baseUrl, 'api/state');

  Uri _uriWithAppendedPath(String baseUrl, String pathToAppend) {
    final uri = Uri.parse(baseUrl.trim());
    final basePath = uri.path.endsWith('/') ? uri.path : '${uri.path}/';
    return uri.replace(
        path: '$basePath$pathToAppend', query: null, fragment: null);
  }

  Map<String, String> _authHeaders(String? token) {
    final headers = <String, String>{'Accept': 'application/json'};
    final trimmed = token?.trim();
    if (trimmed != null && trimmed.isNotEmpty) {
      headers['Authorization'] = 'Bearer $trimmed';
    }
    return headers;
  }

  Map<String, dynamic> _stringKeyMap(Map<dynamic, dynamic> map) {
    return map.map((key, value) => MapEntry(key.toString(), value));
  }

  _LightBoxInventory _inventoryFromState(RhythmHello state) {
    final seen = <String>{};
    var lights = 0;
    var buttons = 0;
    var motionSensors = 0;
    var otherDevices = 0;

    void addDevice(String id, RhythmDeviceType? type) {
      if (id.isEmpty || !seen.add(id)) return;
      switch (type) {
        case RhythmDeviceType.light:
          lights++;
        case RhythmDeviceType.button:
          buttons++;
        case RhythmDeviceType.motion:
          motionSensors++;
        case null:
          otherDevices++;
      }
    }

    for (final node in state.nodes) {
      if (node.kind.isDevice) {
        addDevice(node.id, RhythmDeviceType.fromNodeKind(node.kind));
      }
      for (final device in node.devices) {
        addDevice(device.id, device.type);
      }
    }

    return _LightBoxInventory(
      lights: lights,
      buttons: buttons,
      motionSensors: motionSensors,
      otherDevices: otherDevices,
    );
  }

  Future<void> _sendLoginLink() async {
    final email = _emailController.text.trim();
    if (email.isEmpty) {
      setState(() {
        _authError = 'Enter an email address.';
        _authNotice = null;
      });
      return;
    }

    setState(() {
      _isAuthWorking = true;
      _authError = null;
      _authNotice = null;
    });
    try {
      await AuthService().sendEmailSignInLink(email);
      if (!mounted) return;
      setState(() {
        _authNotice = 'Check your email for a Rhythm login link.';
      });
    } catch (error) {
      if (!mounted) return;
      setState(() {
        _authError = _friendlyAuthError(error);
      });
    } finally {
      if (mounted) {
        setState(() {
          _isAuthWorking = false;
        });
      }
    }
  }

  Future<void> _signOut() async {
    setState(() {
      _isAuthWorking = true;
      _authError = null;
      _authNotice = null;
    });
    try {
      await AuthService().signOut();
      _refresh();
    } catch (error) {
      if (!mounted) return;
      setState(() {
        _authError = _friendlyAuthError(error);
      });
    } finally {
      if (mounted) {
        setState(() {
          _isAuthWorking = false;
        });
      }
    }
  }

  String _friendlyAuthError(Object error) {
    final message = error.toString();
    if (message.toLowerCase().contains('invalid login credentials')) {
      return 'Email did not match an existing account.';
    }
    return message.replaceFirst('Exception: ', '');
  }

  @override
  Widget build(BuildContext context) {
    final auth = AuthService();
    final cloudSync = AccountCloudSyncService.instance;

    return Scaffold(
      backgroundColor: _AdminColors.background,
      body: SafeArea(
        child: Center(
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 1320),
            child: Padding(
              padding: const EdgeInsets.fromLTRB(24, 22, 24, 24),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  _Header(
                    user: auth.currentUser,
                    isWorking: _isAuthWorking,
                    onRefresh: _refresh,
                    onSignOut: _signOut,
                  ),
                  if (!auth.isSignedIn) ...[
                    const SizedBox(height: 16),
                    _AuthPanel(
                      emailController: _emailController,
                      isWorking: _isAuthWorking,
                      error: _authError,
                      notice: _authNotice,
                      onSendLoginLink: _sendLoginLink,
                    ),
                  ],
                  const SizedBox(height: 18),
                  Expanded(
                    child: FutureBuilder<AdminSupportSnapshot>(
                      future: _supportFuture,
                      builder: (context, snapshot) {
                        if (snapshot.connectionState != ConnectionState.done) {
                          return const _Panel(
                            child: Center(child: CircularProgressIndicator()),
                          );
                        }
                        if (snapshot.hasError) {
                          return _Panel(
                            child: _EmptyState(
                              title: 'Could not load cloud homes',
                              detail: snapshot.error.toString(),
                            ),
                          );
                        }
                        if (!auth.isSignedIn) {
                          return const _Panel(
                            child: _EmptyState(
                              title: 'Signed out',
                              detail: 'Sign in to view homes.',
                            ),
                          );
                        }
                        if (!cloudSync.canUseSignedInCloudFeatures) {
                          return const _Panel(
                            child: _EmptyState(
                              title: 'Cloud account access is not active',
                              detail:
                                  'Sign in with a non-anonymous account to view homes saved in Supabase.',
                            ),
                          );
                        }
                        final support =
                            snapshot.data ?? AdminSupportSnapshot.empty;
                        return _SupportWorkspace(
                          support: support,
                          searchController: _searchController,
                          showEmptyHomes: _showEmptyHomes,
                          selectedHomeId: _selectedHomeId,
                          recentHomeIds: _recentHomeIds,
                          liveStatuses: _liveStatuses,
                          maxResults: _maxResults,
                          onShowEmptyHomesChanged: (value) {
                            setState(() => _showEmptyHomes = value);
                          },
                          onSelectHome: _selectHome,
                          onOpenLightBox: _openLightBox,
                        );
                      },
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _Header extends StatelessWidget {
  const _Header({
    required this.user,
    required this.isWorking,
    required this.onRefresh,
    required this.onSignOut,
  });

  final AuthUser? user;
  final bool isWorking;
  final VoidCallback onRefresh;
  final VoidCallback onSignOut;

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        final compact = constraints.maxWidth < 760;
        final title = const Text(
          'Rhythm Customer Support',
          style: TextStyle(
            color: _AdminColors.text,
            fontSize: 28,
            fontWeight: FontWeight.w700,
          ),
        );
        final actions = _HeaderActions(
          user: user,
          isWorking: isWorking,
          onRefresh: onRefresh,
          onSignOut: onSignOut,
        );
        if (compact) {
          return Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              title,
              const SizedBox(height: 12),
              actions,
            ],
          );
        }
        return Row(
          children: [
            Expanded(child: title),
            actions,
          ],
        );
      },
    );
  }
}

class _HeaderActions extends StatelessWidget {
  const _HeaderActions({
    required this.user,
    required this.isWorking,
    required this.onRefresh,
    required this.onSignOut,
  });

  final AuthUser? user;
  final bool isWorking;
  final VoidCallback onRefresh;
  final VoidCallback onSignOut;

  @override
  Widget build(BuildContext context) {
    final signedInUser = user;
    return Wrap(
      spacing: 10,
      runSpacing: 10,
      alignment: WrapAlignment.end,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        if (signedInUser != null)
          _AdminIdentityChip(
            email: signedInUser.email ?? 'Signed in',
            isWorking: isWorking,
            onRefresh: onRefresh,
            onSignOut: onSignOut,
          ),
      ],
    );
  }
}

enum _AccountMenuAction {
  refresh,
  signOut,
}

class _AdminIdentityChip extends StatelessWidget {
  const _AdminIdentityChip({
    required this.email,
    required this.isWorking,
    required this.onRefresh,
    required this.onSignOut,
  });

  final String email;
  final bool isWorking;
  final VoidCallback onRefresh;
  final VoidCallback onSignOut;

  @override
  Widget build(BuildContext context) {
    return PopupMenuButton<_AccountMenuAction>(
      enabled: !isWorking,
      color: _AdminColors.surface,
      surfaceTintColor: Colors.transparent,
      tooltip: 'Account',
      offset: const Offset(0, 8),
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(8),
        side: const BorderSide(color: _AdminColors.border),
      ),
      onSelected: (action) {
        switch (action) {
          case _AccountMenuAction.refresh:
            onRefresh();
          case _AccountMenuAction.signOut:
            onSignOut();
        }
      },
      itemBuilder: (context) => const [
        PopupMenuItem(
          value: _AccountMenuAction.refresh,
          child: _AccountMenuItem(
            icon: Icons.refresh_rounded,
            label: 'Refresh data',
          ),
        ),
        PopupMenuDivider(height: 1),
        PopupMenuItem(
          value: _AccountMenuAction.signOut,
          child: _AccountMenuItem(
            icon: Icons.logout_rounded,
            label: 'Sign out',
          ),
        ),
      ],
      child: DecoratedBox(
        decoration: BoxDecoration(
          color: _AdminColors.surface,
          borderRadius: BorderRadius.circular(8),
          border: Border.all(color: _AdminColors.borderMuted),
        ),
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 9),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              const Icon(
                Icons.verified_user_outlined,
                color: _AdminColors.muted,
                size: 18,
              ),
              const SizedBox(width: 8),
              ConstrainedBox(
                constraints: const BoxConstraints(maxWidth: 280),
                child: Text(
                  email,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(
                    color: _AdminColors.text,
                    fontSize: 14,
                    fontWeight: FontWeight.w700,
                  ),
                ),
              ),
              const SizedBox(width: 6),
              const Icon(
                Icons.keyboard_arrow_down_rounded,
                color: _AdminColors.muted,
                size: 18,
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _SupportWorkspace extends StatelessWidget {
  const _SupportWorkspace({
    required this.support,
    required this.searchController,
    required this.showEmptyHomes,
    required this.selectedHomeId,
    required this.recentHomeIds,
    required this.liveStatuses,
    required this.maxResults,
    required this.onShowEmptyHomesChanged,
    required this.onSelectHome,
    required this.onOpenLightBox,
  });

  final AdminSupportSnapshot support;
  final TextEditingController searchController;
  final bool showEmptyHomes;
  final String? selectedHomeId;
  final List<String> recentHomeIds;
  final Map<String, _LightBoxLiveStatus> liveStatuses;
  final int maxResults;
  final ValueChanged<bool> onShowEmptyHomesChanged;
  final ValueChanged<_HomeSearchResult> onSelectHome;
  final void Function(
          BuildContext, _HomeSearchResult, Hub, _LightBoxLiveStatus?)
      onOpenLightBox;

  @override
  Widget build(BuildContext context) {
    final allResults = _allHomeResults(support.customers);
    final query = searchController.text.trim();
    final filtered = _filteredResults(
      results: allResults,
      query: query,
      showEmptyHomes: showEmptyHomes,
      maxResults: maxResults,
    );
    final selected = _selectedResult(
      results: allResults,
      selectedHomeId: selectedHomeId,
    );
    final recentResults = _recentResults(
      results: allResults,
      recentHomeIds: recentHomeIds,
      showEmptyHomes: showEmptyHomes,
    );

    if (support.customers.isEmpty) {
      return const _Panel(
        child: _EmptyState(
          title: 'No saved homes found',
          detail: 'No cloud-synced homes are visible to this account yet.',
        ),
      );
    }

    return LayoutBuilder(
      builder: (context, constraints) {
        final compact = constraints.maxWidth < 900;
        final rail = _SearchRail(
          searchController: searchController,
          query: query,
          results: filtered,
          totalMatched: _matchedCount(
            results: allResults,
            query: query,
            showEmptyHomes: showEmptyHomes,
          ),
          totalVisible: _visibleHomeCount(
            results: allResults,
            showEmptyHomes: showEmptyHomes,
          ),
          showEmptyHomes: showEmptyHomes,
          selectedHomeId: selectedHomeId,
          recentResults: recentResults,
          liveStatuses: liveStatuses,
          maxResults: maxResults,
          staffStatus: support.staffStatus,
          onShowEmptyHomesChanged: onShowEmptyHomesChanged,
          onSelectHome: onSelectHome,
        );
        final detail = _DetailPane(
          selected: selected,
          liveStatuses: liveStatuses,
          onOpenLightBox: onOpenLightBox,
        );

        if (compact) {
          return Column(
            children: [
              SizedBox(height: 310, child: rail),
              const SizedBox(height: 16),
              Expanded(child: detail),
            ],
          );
        }

        return Row(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            SizedBox(width: 390, child: rail),
            const SizedBox(width: 22),
            Expanded(child: detail),
          ],
        );
      },
    );
  }
}

class _SearchRail extends StatelessWidget {
  const _SearchRail({
    required this.searchController,
    required this.query,
    required this.results,
    required this.totalMatched,
    required this.totalVisible,
    required this.showEmptyHomes,
    required this.selectedHomeId,
    required this.recentResults,
    required this.liveStatuses,
    required this.maxResults,
    required this.staffStatus,
    required this.onShowEmptyHomesChanged,
    required this.onSelectHome,
  });

  final TextEditingController searchController;
  final String query;
  final List<_HomeSearchResult> results;
  final int totalMatched;
  final int totalVisible;
  final bool showEmptyHomes;
  final String? selectedHomeId;
  final List<_HomeSearchResult> recentResults;
  final Map<String, _LightBoxLiveStatus> liveStatuses;
  final int maxResults;
  final SupportStaffStatus staffStatus;
  final ValueChanged<bool> onShowEmptyHomesChanged;
  final ValueChanged<_HomeSearchResult> onSelectHome;

  @override
  Widget build(BuildContext context) {
    return _Panel(
      padding: const EdgeInsets.all(14),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          TextField(
            controller: searchController,
            autofocus: true,
            style: const TextStyle(color: _AdminColors.text),
            decoration: InputDecoration(
              hintText: 'Search email, name, city, home, LightBox',
              hintStyle: const TextStyle(color: _AdminColors.dim),
              prefixIcon: const Icon(
                Icons.search_rounded,
                color: _AdminColors.muted,
              ),
              suffixIcon: query.isEmpty
                  ? null
                  : IconButton(
                      tooltip: 'Clear search',
                      onPressed: searchController.clear,
                      icon: const Icon(
                        Icons.close_rounded,
                        color: _AdminColors.muted,
                      ),
                    ),
              filled: true,
              fillColor: _AdminColors.field,
              enabledBorder: OutlineInputBorder(
                borderRadius: BorderRadius.circular(8),
                borderSide: const BorderSide(color: _AdminColors.borderMuted),
              ),
              focusedBorder: OutlineInputBorder(
                borderRadius: BorderRadius.circular(8),
                borderSide: const BorderSide(color: _AdminColors.warning),
              ),
            ),
          ),
          const SizedBox(height: 10),
          _RailOptions(
            showEmptyHomes: showEmptyHomes,
            onChanged: onShowEmptyHomesChanged,
          ),
          const SizedBox(height: 8),
          if (!staffStatus.isActive) ...[
            _StaffAccessNotice(status: staffStatus),
            const SizedBox(height: 10),
          ],
          Expanded(
            child: query.isEmpty
                ? _SearchLanding(
                    totalVisible: totalVisible,
                    recentResults: recentResults,
                    selectedHomeId: selectedHomeId,
                    liveStatuses: liveStatuses,
                    onSelectHome: onSelectHome,
                  )
                : _SearchResultsList(
                    results: results,
                    totalMatched: totalMatched,
                    selectedHomeId: selectedHomeId,
                    liveStatuses: liveStatuses,
                    maxResults: maxResults,
                    onSelectHome: onSelectHome,
                  ),
          ),
        ],
      ),
    );
  }
}

class _RailOptions extends StatelessWidget {
  const _RailOptions({
    required this.showEmptyHomes,
    required this.onChanged,
  });

  final bool showEmptyHomes;
  final ValueChanged<bool> onChanged;

  @override
  Widget build(BuildContext context) {
    return InkWell(
      borderRadius: BorderRadius.circular(8),
      onTap: () => onChanged(!showEmptyHomes),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 2, vertical: 4),
        child: Row(
          children: [
            Checkbox(
              value: showEmptyHomes,
              onChanged: (value) => onChanged(value ?? false),
              activeColor: _AdminColors.warning,
              checkColor: _AdminColors.background,
              side: const BorderSide(color: _AdminColors.border),
              materialTapTargetSize: MaterialTapTargetSize.shrinkWrap,
            ),
            const SizedBox(width: 6),
            const Expanded(
              child: Text(
                'includes homes without a LightBox',
                style: TextStyle(
                  color: _AdminColors.muted,
                  fontSize: 13,
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _SearchLanding extends StatelessWidget {
  const _SearchLanding({
    required this.totalVisible,
    required this.recentResults,
    required this.selectedHomeId,
    required this.liveStatuses,
    required this.onSelectHome,
  });

  final int totalVisible;
  final List<_HomeSearchResult> recentResults;
  final String? selectedHomeId;
  final Map<String, _LightBoxLiveStatus> liveStatuses;
  final ValueChanged<_HomeSearchResult> onSelectHome;

  @override
  Widget build(BuildContext context) {
    if (recentResults.isEmpty) {
      return Center(
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 280),
          child: Text(
            _homeCountText(totalVisible),
            textAlign: TextAlign.center,
            style: const TextStyle(
              color: _AdminColors.muted,
              fontSize: 14,
              height: 1.35,
            ),
          ),
        ),
      );
    }

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const Padding(
          padding: EdgeInsets.fromLTRB(2, 4, 2, 8),
          child: Text(
            'Recently accessed',
            style: TextStyle(
              color: _AdminColors.dim,
              fontSize: 12,
              fontWeight: FontWeight.w700,
            ),
          ),
        ),
        Expanded(
          child: ListView.builder(
            itemCount: recentResults.length,
            itemBuilder: (context, index) {
              final result = recentResults[index];
              return _SearchResultRow(
                result: result,
                selected: result.home.id == selectedHomeId,
                statusColor: _homeDotColor(result, liveStatuses),
                onTap: () => onSelectHome(result),
              );
            },
          ),
        ),
        Padding(
          padding: const EdgeInsets.only(top: 8),
          child: Text(
            _homeCountText(totalVisible),
            style: const TextStyle(
              color: _AdminColors.dim,
              fontSize: 12,
              height: 1.35,
            ),
          ),
        ),
      ],
    );
  }
}

class _SearchResultsList extends StatelessWidget {
  const _SearchResultsList({
    required this.results,
    required this.totalMatched,
    required this.selectedHomeId,
    required this.liveStatuses,
    required this.maxResults,
    required this.onSelectHome,
  });

  final List<_HomeSearchResult> results;
  final int totalMatched;
  final String? selectedHomeId;
  final Map<String, _LightBoxLiveStatus> liveStatuses;
  final int maxResults;
  final ValueChanged<_HomeSearchResult> onSelectHome;

  @override
  Widget build(BuildContext context) {
    if (results.isEmpty) {
      return const _EmptyState(
        title: 'No matches',
        detail: 'Try a different customer, city, home, or LightBox.',
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(2, 0, 2, 8),
          child: Text(
            totalMatched > maxResults
                ? '$maxResults of $totalMatched matches'
                : '$totalMatched matches',
            style: const TextStyle(
              color: _AdminColors.dim,
              fontSize: 12,
              fontWeight: FontWeight.w600,
            ),
          ),
        ),
        Expanded(
          child: ListView.builder(
            itemCount: results.length,
            itemBuilder: (context, index) {
              final result = results[index];
              return _SearchResultRow(
                result: result,
                selected: result.home.id == selectedHomeId,
                statusColor: _homeDotColor(result, liveStatuses),
                onTap: () => onSelectHome(result),
              );
            },
          ),
        ),
      ],
    );
  }
}

class _SearchResultRow extends StatelessWidget {
  const _SearchResultRow({
    required this.result,
    required this.selected,
    required this.statusColor,
    required this.onTap,
  });

  final _HomeSearchResult result;
  final bool selected;
  final Color statusColor;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final opacity = result.hasLightBoxes ? 1.0 : 0.58;
    return Padding(
      padding: const EdgeInsets.only(bottom: 6),
      child: InkWell(
        borderRadius: BorderRadius.circular(8),
        onTap: onTap,
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 120),
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 10),
          decoration: BoxDecoration(
            color: selected ? _AdminColors.surfaceMuted : Colors.transparent,
            borderRadius: BorderRadius.circular(8),
            border: Border.all(
              color: selected ? _AdminColors.border : Colors.transparent,
            ),
          ),
          child: Opacity(
            opacity: opacity,
            child: Row(
              children: [
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        result.customerTitle,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: const TextStyle(
                          color: _AdminColors.text,
                          fontSize: 14,
                          fontWeight: FontWeight.w700,
                        ),
                      ),
                      const SizedBox(height: 3),
                      Text(
                        result.homeLabel,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: const TextStyle(
                          color: _AdminColors.muted,
                          fontSize: 13,
                        ),
                      ),
                    ],
                  ),
                ),
                const SizedBox(width: 10),
                _StatusDot(color: statusColor),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _DetailPane extends StatelessWidget {
  const _DetailPane({
    required this.selected,
    required this.liveStatuses,
    required this.onOpenLightBox,
  });

  final _HomeSearchResult? selected;
  final Map<String, _LightBoxLiveStatus> liveStatuses;
  final void Function(
          BuildContext, _HomeSearchResult, Hub, _LightBoxLiveStatus?)
      onOpenLightBox;

  @override
  Widget build(BuildContext context) {
    final result = selected;
    if (result == null) {
      return const _Panel(
        child: _EmptyState(
          title: 'Select a home',
        ),
      );
    }

    return _Panel(
      padding: const EdgeInsets.all(20),
      child: ListView(
        children: [
          _SelectedHomeHeader(result: result),
          const SizedBox(height: 18),
          if (result.lightBoxes.isEmpty)
            const _NoLightBoxesDetail()
          else
            for (final box in result.lightBoxes) ...[
              _LightBoxCard(
                result: result,
                hub: box,
                liveStatus: liveStatuses[box.id],
                onOpen: () =>
                    onOpenLightBox(context, result, box, liveStatuses[box.id]),
              ),
              const SizedBox(height: 12),
            ],
          const SizedBox(height: 4),
          _SupportDetailsDisclosure(
            result: result,
            liveStatuses: liveStatuses,
          ),
        ],
      ),
    );
  }
}

class _SelectedHomeHeader extends StatelessWidget {
  const _SelectedHomeHeader({
    required this.result,
  });

  final _HomeSearchResult result;

  @override
  Widget build(BuildContext context) {
    final subtitle = [
      result.customerTitle,
      if (result.customerSubtitle != null) result.customerSubtitle!,
    ].join(' · ');
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          result.homeLabel,
          maxLines: 2,
          overflow: TextOverflow.ellipsis,
          style: const TextStyle(
            color: _AdminColors.text,
            fontSize: 24,
            fontWeight: FontWeight.w800,
          ),
        ),
        const SizedBox(height: 6),
        Text(
          subtitle,
          maxLines: 2,
          overflow: TextOverflow.ellipsis,
          style: const TextStyle(
            color: _AdminColors.muted,
            fontSize: 14,
          ),
        ),
      ],
    );
  }
}

class _NoLightBoxesDetail extends StatelessWidget {
  const _NoLightBoxesDetail();

  @override
  Widget build(BuildContext context) {
    return const Padding(
      padding: EdgeInsets.symmetric(vertical: 18),
      child: Text(
        'No LightBoxes saved for this home.',
        style: TextStyle(
          color: _AdminColors.muted,
          fontSize: 14,
        ),
      ),
    );
  }
}

class _LightBoxCard extends StatelessWidget {
  const _LightBoxCard({
    required this.result,
    required this.hub,
    required this.liveStatus,
    required this.onOpen,
  });

  final _HomeSearchResult result;
  final Hub hub;
  final _LightBoxLiveStatus? liveStatus;
  final VoidCallback onOpen;

  @override
  Widget build(BuildContext context) {
    return DecoratedBox(
      decoration: BoxDecoration(
        color: _AdminColors.field,
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: _AdminColors.borderMuted),
      ),
      child: Padding(
        padding: const EdgeInsets.all(14),
        child: LayoutBuilder(
          builder: (context, constraints) {
            final compact = constraints.maxWidth < 680;
            final identity = Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Wrap(
                  spacing: 8,
                  runSpacing: 6,
                  crossAxisAlignment: WrapCrossAlignment.center,
                  children: [
                    Text(
                      result.lightBoxes.length == 1 ? 'LightBox' : hub.name,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(
                        color: _AdminColors.text,
                        fontSize: 16,
                        fontWeight: FontWeight.w800,
                      ),
                    ),
                    _StatusPill(liveStatus: liveStatus),
                  ],
                ),
                const SizedBox(height: 8),
                Text(
                  _deviceCountLabel(liveStatus?.inventory),
                  style: const TextStyle(
                    color: _AdminColors.muted,
                    fontSize: 13,
                  ),
                ),
              ],
            );
            final action = _OpenLightBoxButton(onOpen: onOpen);

            if (compact) {
              return Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  identity,
                  const SizedBox(height: 12),
                  Align(alignment: Alignment.centerLeft, child: action),
                ],
              );
            }

            return Row(
              crossAxisAlignment: CrossAxisAlignment.center,
              children: [
                Expanded(child: identity),
                const SizedBox(width: 16),
                action,
              ],
            );
          },
        ),
      ),
    );
  }
}

class _OpenLightBoxButton extends StatelessWidget {
  const _OpenLightBoxButton({required this.onOpen});

  final VoidCallback onOpen;

  @override
  Widget build(BuildContext context) {
    return FilledButton.icon(
      onPressed: onOpen,
      style: FilledButton.styleFrom(
        backgroundColor: _AdminColors.warning,
        foregroundColor: _AdminColors.background,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(8),
        ),
        padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 14),
      ),
      icon: const Icon(Icons.router_outlined),
      label: const Text('Open LightBox'),
    );
  }
}

class _SupportDetailsDisclosure extends StatefulWidget {
  const _SupportDetailsDisclosure({
    required this.result,
    required this.liveStatuses,
  });

  final _HomeSearchResult result;
  final Map<String, _LightBoxLiveStatus> liveStatuses;

  @override
  State<_SupportDetailsDisclosure> createState() =>
      _SupportDetailsDisclosureState();
}

class _SupportDetailsDisclosureState extends State<_SupportDetailsDisclosure> {
  bool _expanded = false;

  @override
  Widget build(BuildContext context) {
    final result = widget.result;
    final liveStatuses = widget.liveStatuses;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTap: () => setState(() => _expanded = !_expanded),
          child: Padding(
            padding: const EdgeInsets.symmetric(vertical: 14),
            child: Row(
              children: [
                const Expanded(
                  child: Text(
                    'Support details',
                    style: TextStyle(
                      color: _AdminColors.muted,
                      fontSize: 14,
                      fontWeight: FontWeight.w700,
                    ),
                  ),
                ),
                Icon(
                  _expanded
                      ? Icons.keyboard_arrow_up_rounded
                      : Icons.keyboard_arrow_down_rounded,
                  color: _AdminColors.muted,
                ),
              ],
            ),
          ),
        ),
        if (_expanded) ...[
          const SizedBox(height: 8),
          _FieldGroup(
            children: [
              _FieldRow(label: 'Customer', value: result.customerTitle),
              if (result.customerSubtitle != null)
                _FieldRow(label: 'Email', value: result.customerSubtitle!),
              _FieldRow(label: 'Home', value: result.home.name),
              _FieldRow(label: 'Location', value: result.locationLabel),
              _FieldRow(
                  label: 'Timezone', value: result.home.timezone ?? 'Unset'),
              _FieldRow(label: 'Home ID', value: result.home.id),
              _FieldRow(label: 'Owner ID', value: result.home.ownerId),
              _FieldRow(
                label: 'Member IDs',
                value: result.home.memberIds.isEmpty
                    ? 'None'
                    : result.home.memberIds.join(', '),
              ),
              _FieldRow(
                  label: 'Created',
                  value: _formatDateTime(result.home.createdAt)),
              _FieldRow(
                  label: 'Updated',
                  value: _formatDateTime(result.home.updatedAt)),
            ],
          ),
          for (final box in result.lightBoxes) ...[
            const SizedBox(height: 14),
            _FieldGroup(
              title: result.lightBoxes.length == 1 ? 'LightBox' : box.name,
              children: [
                _FieldRow(
                  label: 'Status',
                  value: _statusLabel(liveStatuses[box.id]),
                ),
                _FieldRow(
                  label: 'Devices',
                  value: _deviceDetailLabel(liveStatuses[box.id]?.inventory),
                ),
                _FieldRow(label: 'LightBox ID', value: box.id),
                _FieldRow(label: 'Local address', value: box.endpoint.baseUrl),
                _FieldRow(
                  label: 'Remote address',
                  value: box.remoteEndpoint?.baseUrl ?? 'Not set up',
                ),
                _FieldRow(
                  label: 'Last seen',
                  value: _formatSupportDateTime(box.lastConnected),
                ),
                _FieldRow(
                  label: 'Server instance',
                  value: box.serverInstanceId ?? 'None',
                ),
                _FieldRow(
                    label: 'Token present',
                    value: _formatBool(box.token != null)),
                _FieldRow(label: 'Type', value: _hubTypeLabel(box.type)),
                _FieldRow(
                    label: 'Updated', value: _formatDateTime(box.updatedAt)),
              ],
            ),
          ],
        ],
      ],
    );
  }
}

Future<void> _showOpenLightBoxDialog(
  BuildContext context,
  _HomeSearchResult result,
  Hub hub,
  _LightBoxLiveStatus? liveStatus,
) {
  return showDialog<void>(
    context: context,
    builder: (context) => _SupportDialog(
      title: 'Open LightBox',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          const Text(
            'This will become the support view for rooms, lighting controls, settings, and troubleshooting.',
            style: TextStyle(
              color: _AdminColors.muted,
              fontSize: 13,
              height: 1.35,
            ),
          ),
          const SizedBox(height: 14),
          _FieldGroup(
            children: [
              _FieldRow(label: 'Customer', value: result.customerTitle),
              if (result.customerSubtitle != null)
                _FieldRow(label: 'Email', value: result.customerSubtitle!),
              _FieldRow(label: 'Home', value: result.home.name),
              _FieldRow(
                label: 'LightBox',
                value: result.lightBoxes.length == 1 ? 'LightBox' : hub.name,
              ),
              _FieldRow(label: 'Status', value: _statusLabel(liveStatus)),
              _FieldRow(
                label: 'Devices',
                value: _deviceDetailLabel(liveStatus?.inventory),
              ),
            ],
          ),
        ],
      ),
    ),
  );
}

class _StatusPill extends StatelessWidget {
  const _StatusPill({required this.liveStatus});

  final _LightBoxLiveStatus? liveStatus;

  @override
  Widget build(BuildContext context) {
    return DecoratedBox(
      decoration: BoxDecoration(
        color: _statusPillBackground(liveStatus),
        borderRadius: BorderRadius.circular(999),
        border: Border.all(color: _statusPillBorder(liveStatus)),
      ),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 5),
        child: Text(
          _statusLabel(liveStatus),
          style: TextStyle(
            color: _statusPillTextColor(liveStatus),
            fontSize: 12,
            fontWeight: FontWeight.w700,
          ),
        ),
      ),
    );
  }
}

class _StatusDot extends StatelessWidget {
  const _StatusDot({required this.color});

  final Color color;

  @override
  Widget build(BuildContext context) {
    return DecoratedBox(
      decoration: BoxDecoration(
        color: color,
        shape: BoxShape.circle,
      ),
      child: const SizedBox.square(dimension: 9),
    );
  }
}

class _Panel extends StatelessWidget {
  const _Panel({required this.child, this.padding = const EdgeInsets.all(16)});

  final Widget child;
  final EdgeInsetsGeometry padding;

  @override
  Widget build(BuildContext context) {
    return DecoratedBox(
      decoration: BoxDecoration(
        color: _AdminColors.surface,
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: _AdminColors.border),
      ),
      child: Padding(padding: padding, child: child),
    );
  }
}

class _AuthPanel extends StatelessWidget {
  const _AuthPanel({
    required this.emailController,
    required this.isWorking,
    required this.error,
    required this.notice,
    required this.onSendLoginLink,
  });

  final TextEditingController emailController;
  final bool isWorking;
  final String? error;
  final String? notice;
  final VoidCallback onSendLoginLink;

  @override
  Widget build(BuildContext context) {
    return _Panel(
      padding: const EdgeInsets.all(14),
      child: _SignInForm(
        emailController: emailController,
        isWorking: isWorking,
        error: error,
        notice: notice,
        onSendLoginLink: onSendLoginLink,
      ),
    );
  }
}

class _SignInForm extends StatelessWidget {
  const _SignInForm({
    required this.emailController,
    required this.isWorking,
    required this.error,
    required this.notice,
    required this.onSendLoginLink,
  });

  final TextEditingController emailController;
  final bool isWorking;
  final String? error;
  final String? notice;
  final VoidCallback onSendLoginLink;

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        final compact = constraints.maxWidth < 720;
        final emailField = _AdminTextField(
          controller: emailController,
          label: 'Email',
          keyboardType: TextInputType.emailAddress,
          enabled: !isWorking,
          onSubmitted: (_) => onSendLoginLink(),
        );
        final signInButton = SizedBox(
          height: 48,
          child: FilledButton.icon(
            onPressed: isWorking ? null : onSendLoginLink,
            style: FilledButton.styleFrom(
              backgroundColor: _AdminColors.surfaceMuted,
              foregroundColor: _AdminColors.text,
              disabledBackgroundColor: _AdminColors.border,
              disabledForegroundColor: _AdminColors.dim,
              shape: RoundedRectangleBorder(
                borderRadius: BorderRadius.circular(8),
              ),
            ),
            icon: isWorking
                ? const SizedBox.square(
                    dimension: 16,
                    child: CircularProgressIndicator(strokeWidth: 2),
                  )
                : const Icon(Icons.login_rounded),
            label: const Text('Email me a login link'),
          ),
        );

        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            compact
                ? Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      emailField,
                      const SizedBox(height: 10),
                      signInButton,
                    ],
                  )
                : Row(
                    children: [
                      Expanded(child: emailField),
                      const SizedBox(width: 10),
                      signInButton,
                    ],
                  ),
            if (notice != null) ...[
              const SizedBox(height: 10),
              Text(
                notice!,
                style: const TextStyle(
                  color: _AdminColors.accent,
                  fontSize: 13,
                ),
              ),
            ],
            if (error != null) ...[
              const SizedBox(height: 10),
              Text(
                error!,
                style: const TextStyle(
                  color: _AdminColors.danger,
                  fontSize: 13,
                ),
              ),
            ],
          ],
        );
      },
    );
  }
}

class _AdminTextField extends StatelessWidget {
  const _AdminTextField({
    required this.controller,
    required this.label,
    required this.enabled,
    this.keyboardType,
    this.onSubmitted,
  });

  final TextEditingController controller;
  final String label;
  final bool enabled;
  final TextInputType? keyboardType;
  final ValueChanged<String>? onSubmitted;

  @override
  Widget build(BuildContext context) {
    return TextField(
      controller: controller,
      enabled: enabled,
      keyboardType: keyboardType,
      onSubmitted: onSubmitted,
      style: const TextStyle(color: _AdminColors.text),
      decoration: InputDecoration(
        labelText: label,
        labelStyle: const TextStyle(color: _AdminColors.muted),
        filled: true,
        fillColor: _AdminColors.field,
        enabledBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(8),
          borderSide: const BorderSide(color: _AdminColors.borderMuted),
        ),
        focusedBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(8),
          borderSide: const BorderSide(color: _AdminColors.warning),
        ),
        disabledBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(8),
          borderSide: const BorderSide(color: _AdminColors.borderMuted),
        ),
      ),
    );
  }
}

class _AccountMenuItem extends StatelessWidget {
  const _AccountMenuItem({
    required this.icon,
    required this.label,
  });

  final IconData icon;
  final String label;

  @override
  Widget build(BuildContext context) {
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Icon(icon, color: _AdminColors.muted, size: 18),
        const SizedBox(width: 10),
        Text(
          label,
          style: const TextStyle(
            color: _AdminColors.text,
            fontSize: 13,
            fontWeight: FontWeight.w600,
          ),
        ),
      ],
    );
  }
}

class _StaffAccessNotice extends StatelessWidget {
  const _StaffAccessNotice({required this.status});

  final SupportStaffStatus status;

  @override
  Widget build(BuildContext context) {
    final detail = status.isUnknown
        ? 'Could not confirm staff access. Showing the homes Supabase allows this sign-in to see.'
        : 'Staff access is not active for this sign-in. Showing only homes tied to this account.';
    return Text(
      detail,
      style: const TextStyle(
        color: _AdminColors.muted,
        fontSize: 13,
        height: 1.35,
      ),
    );
  }
}

class _EmptyState extends StatelessWidget {
  const _EmptyState({
    required this.title,
    this.detail,
  });

  final String title;
  final String? detail;

  @override
  Widget build(BuildContext context) {
    return Center(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 360),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(
              title,
              textAlign: TextAlign.center,
              style: const TextStyle(
                color: _AdminColors.text,
                fontSize: 17,
                fontWeight: FontWeight.w800,
              ),
            ),
            if (detail != null) ...[
              const SizedBox(height: 8),
              Text(
                detail!,
                textAlign: TextAlign.center,
                style: const TextStyle(
                  color: _AdminColors.muted,
                  fontSize: 14,
                  height: 1.35,
                ),
              ),
            ],
          ],
        ),
      ),
    );
  }
}

class _SupportDialog extends StatelessWidget {
  const _SupportDialog({
    required this.title,
    required this.child,
  });

  final String title;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    return Dialog(
      backgroundColor: _AdminColors.surface,
      surfaceTintColor: Colors.transparent,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(8),
        side: const BorderSide(color: _AdminColors.border),
      ),
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 560),
        child: Padding(
          padding: const EdgeInsets.all(18),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Row(
                children: [
                  Expanded(
                    child: Text(
                      title,
                      style: const TextStyle(
                        color: _AdminColors.text,
                        fontSize: 18,
                        fontWeight: FontWeight.w800,
                      ),
                    ),
                  ),
                  IconButton(
                    tooltip: 'Close',
                    onPressed: () => Navigator.of(context).pop(),
                    icon: const Icon(
                      Icons.close_rounded,
                      color: _AdminColors.muted,
                    ),
                  ),
                ],
              ),
              const SizedBox(height: 10),
              child,
            ],
          ),
        ),
      ),
    );
  }
}

class _FieldGroup extends StatelessWidget {
  const _FieldGroup({
    required this.children,
    this.title,
  });

  final String? title;
  final List<Widget> children;

  @override
  Widget build(BuildContext context) {
    return DecoratedBox(
      decoration: BoxDecoration(
        color: _AdminColors.surfaceMuted,
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: _AdminColors.borderMuted),
      ),
      child: Padding(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (title != null) ...[
              Text(
                title!,
                style: const TextStyle(
                  color: _AdminColors.text,
                  fontSize: 13,
                  fontWeight: FontWeight.w800,
                ),
              ),
              const SizedBox(height: 10),
            ],
            for (var index = 0; index < children.length; index++) ...[
              if (index != 0)
                const Divider(height: 16, color: _AdminColors.borderMuted),
              children[index],
            ],
          ],
        ),
      ),
    );
  }
}

class _FieldRow extends StatelessWidget {
  const _FieldRow({
    required this.label,
    required this.value,
  });

  final String label;
  final String value;

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        final compact = constraints.maxWidth < 520;
        final labelWidget = Text(
          label,
          style: const TextStyle(
            color: _AdminColors.dim,
            fontSize: 12,
            fontWeight: FontWeight.w800,
          ),
        );
        final valueWidget = SelectableText(
          value,
          style: const TextStyle(
            color: _AdminColors.text,
            fontSize: 13,
            height: 1.25,
          ),
        );
        if (compact) {
          return Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              labelWidget,
              const SizedBox(height: 4),
              valueWidget,
            ],
          );
        }
        return Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            SizedBox(width: 140, child: labelWidget),
            const SizedBox(width: 12),
            Expanded(child: valueWidget),
          ],
        );
      },
    );
  }
}

List<_HomeSearchResult> _allHomeResults(List<SupportCustomerHomes> customers) {
  return [
    for (final customer in customers)
      for (final snapshot in customer.homes)
        _HomeSearchResult(customer: customer, snapshot: snapshot),
  ];
}

List<_HomeSearchResult> _recentResults({
  required List<_HomeSearchResult> results,
  required List<String> recentHomeIds,
  required bool showEmptyHomes,
}) {
  if (recentHomeIds.isEmpty) return const [];
  final byHomeId = {
    for (final result in results)
      if (showEmptyHomes || result.hasLightBoxes) result.home.id: result,
  };
  return [
    for (final id in recentHomeIds)
      if (byHomeId[id] != null) byHomeId[id]!,
  ];
}

List<_HomeSearchResult> _filteredResults({
  required List<_HomeSearchResult> results,
  required String query,
  required bool showEmptyHomes,
  required int maxResults,
}) {
  if (query.trim().isEmpty) return const [];
  final terms = query
      .trim()
      .toLowerCase()
      .split(RegExp(r'\s+'))
      .where((term) => term.isNotEmpty)
      .toList(growable: false);
  final filtered = results.where((result) {
    if (!showEmptyHomes && !result.hasLightBoxes) return false;
    final searchText = result.searchText;
    return terms.every(searchText.contains);
  }).toList(growable: false);
  filtered.sort(_sortSearchResults);
  return filtered.take(maxResults).toList(growable: false);
}

int _matchedCount({
  required List<_HomeSearchResult> results,
  required String query,
  required bool showEmptyHomes,
}) {
  if (query.trim().isEmpty) return 0;
  final terms = query
      .trim()
      .toLowerCase()
      .split(RegExp(r'\s+'))
      .where((term) => term.isNotEmpty)
      .toList(growable: false);
  return results.where((result) {
    if (!showEmptyHomes && !result.hasLightBoxes) return false;
    final searchText = result.searchText;
    return terms.every(searchText.contains);
  }).length;
}

int _visibleHomeCount({
  required List<_HomeSearchResult> results,
  required bool showEmptyHomes,
}) {
  if (showEmptyHomes) return results.length;
  return results.where((result) => result.hasLightBoxes).length;
}

String _homeCountText(int count) {
  return '$count ${count == 1 ? 'home' : 'homes'}';
}

int _sortSearchResults(_HomeSearchResult left, _HomeSearchResult right) {
  if (left.hasLightBoxes != right.hasLightBoxes) {
    return left.hasLightBoxes ? -1 : 1;
  }
  final customerCompare = left.customerTitle
      .toLowerCase()
      .compareTo(right.customerTitle.toLowerCase());
  if (customerCompare != 0) return customerCompare;
  return left.homeLabel.toLowerCase().compareTo(right.homeLabel.toLowerCase());
}

_HomeSearchResult? _selectedResult({
  required List<_HomeSearchResult> results,
  required String? selectedHomeId,
}) {
  if (selectedHomeId == null) return null;
  for (final result in results) {
    if (result.home.id == selectedHomeId) return result;
  }
  return null;
}

Color _homeDotColor(
  _HomeSearchResult result,
  Map<String, _LightBoxLiveStatus> liveStatuses,
) {
  if (result.lightBoxes.isEmpty) return _AdminColors.dim;
  final statuses = [
    for (final box in result.lightBoxes) liveStatuses[box.id],
  ];
  if (statuses.any((status) =>
      status?.kind == _LightBoxLiveStatusKind.local ||
      status?.kind == _LightBoxLiveStatusKind.remote)) {
    return _AdminColors.accent;
  }
  if (statuses
      .every((status) => status?.kind == _LightBoxLiveStatusKind.offline)) {
    return _AdminColors.danger;
  }
  if (statuses
      .any((status) => status?.kind == _LightBoxLiveStatusKind.checking)) {
    return _AdminColors.muted;
  }
  return _AdminColors.dim;
}

String _statusLabel(_LightBoxLiveStatus? liveStatus) {
  return switch (liveStatus?.kind) {
    _LightBoxLiveStatusKind.local => 'Online (local)',
    _LightBoxLiveStatusKind.remote => 'Online (remote)',
    _LightBoxLiveStatusKind.offline => 'Offline',
    _LightBoxLiveStatusKind.checking => 'Checking',
    null => 'Not checked',
  };
}

Color _statusPillTextColor(_LightBoxLiveStatus? liveStatus) {
  return switch (liveStatus?.kind) {
    _LightBoxLiveStatusKind.local ||
    _LightBoxLiveStatusKind.remote =>
      _AdminColors.accent,
    _LightBoxLiveStatusKind.offline => _AdminColors.danger,
    _LightBoxLiveStatusKind.checking || null => _AdminColors.muted,
  };
}

Color _statusPillBackground(_LightBoxLiveStatus? liveStatus) {
  return switch (liveStatus?.kind) {
    _LightBoxLiveStatusKind.local ||
    _LightBoxLiveStatusKind.remote =>
      _AdminColors.accentWash,
    _LightBoxLiveStatusKind.offline => _AdminColors.dangerWash,
    _LightBoxLiveStatusKind.checking || null => _AdminColors.neutralWash,
  };
}

Color _statusPillBorder(_LightBoxLiveStatus? liveStatus) {
  return switch (liveStatus?.kind) {
    _LightBoxLiveStatusKind.local ||
    _LightBoxLiveStatusKind.remote =>
      _AdminColors.accent,
    _LightBoxLiveStatusKind.offline => _AdminColors.danger,
    _LightBoxLiveStatusKind.checking || null => _AdminColors.border,
  };
}

String _deviceCountLabel(_LightBoxInventory? inventory) {
  if (inventory == null) return 'Devices ?';
  return _countLabel(inventory.total, 'device');
}

String _deviceDetailLabel(_LightBoxInventory? inventory) {
  if (inventory == null) return 'Unknown';
  final parts = <String>[
    if (inventory.lights > 0) _countLabel(inventory.lights, 'light'),
    if (inventory.buttons > 0) _countLabel(inventory.buttons, 'button'),
    if (inventory.motionSensors > 0)
      _countLabel(inventory.motionSensors, 'sensor'),
    if (inventory.otherDevices > 0)
      _countLabel(inventory.otherDevices, 'other device'),
  ];
  return parts.isEmpty ? '0 devices' : parts.join(', ');
}

String _countLabel(int count, String singular) {
  return '$count $singular${count == 1 ? '' : 's'}';
}

String _homeLocationLabel(Home home) {
  final city = home.location?.cityName?.trim();
  if (city != null && city.isNotEmpty) return city;
  return home.timezone ?? 'Location unset';
}

String _formatBool(bool value) => value ? 'Yes' : 'No';

String _hubTypeLabel(HubType type) {
  return switch (type) {
    HubType.homeAssistant => 'Home Assistant',
    HubType.hue => 'Hue',
    HubType.server => 'LightBox',
  };
}

String _formatSupportDateTime(DateTime? value) {
  if (value == null) return 'Never';
  return _formatDateTime(value);
}

String _formatDateTime(DateTime value) {
  final local = value.toLocal();
  return '${local.year}-${_two(local.month)}-${_two(local.day)} '
      '${_two(local.hour)}:${_two(local.minute)}';
}

String _two(int value) => value.toString().padLeft(2, '0');
