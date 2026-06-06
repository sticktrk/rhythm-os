import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show RhythmConnectionState, RhythmMode;
import 'models/config_model.dart';
import 'providers/room_provider.dart';
import 'providers/home_provider.dart';
import 'providers/room_page_provider.dart';
import 'providers/server_sync_provider.dart';
import 'screens/all_rooms_screen.dart';
import 'screens/server_disconnected_screen.dart';
import 'screens/settings/automations_screen.dart';
import 'screens/settings/light_screen.dart';
import 'screens/settings/sections/lights_devices_section.dart';
import 'screens/settings/settings_screen.dart';
import 'screens/sun_position_screen.dart';
import 'services/analytics_service.dart';
import 'services/app_state_refresh.dart';
import 'services/hue/hue_service_locator.dart';
import 'services/virtual_experience_service.dart';
import 'utils/room_visibility.dart';
import 'widgets/connect_hub_screen.dart';
import 'widgets/hardware_gate_screen.dart';
import 'widgets/hub_picker_screen.dart';
import 'widgets/main_bottom_nav.dart';
import 'widgets/solar_orbit.dart';
import 'widgets/virtual_experience_banner.dart';

/// Main app shell.
///
/// Persistent 5-tab bottom navigation bar with an [IndexedStack] body.  Each
/// tab owns its own [Navigator] so deeper pushes (e.g. Devices → Device Review)
/// stay inside the body slot and the navbar remains visible.
class AppShell extends StatefulWidget {
  const AppShell({super.key});

  @override
  State<AppShell> createState() => _AppShellState();
}

class _AppShellState extends State<AppShell> with WidgetsBindingObserver {
  static const _modeActionSseHandoffGrace = Duration(seconds: 3);

  static const _tabs = [
    MainNavTab.home,
    MainNavTab.light,
    MainNavTab.automations,
    MainNavTab.devices,
    MainNavTab.settings,
  ];

  int _tabIndex = 0;
  // Keyed by tab identity (not index) so reordering [_tabs] doesn't strand
  // a tab's Navigator state on the wrong screen — the initial route attached
  // to a Navigator GlobalKey survives hot reload, so per-index keys would
  // make a swap appear to "lose" screens until a full restart.
  final Set<MainNavTab> _builtTabs = {MainNavTab.home};
  final Map<MainNavTab, GlobalKey<NavigatorState>> _navKeys = {
    for (final t in MainNavTab.values) t: GlobalKey<NavigatorState>(),
  };

  CurveData? _curveData;
  final PageController _roomPageController = PageController();
  bool _hadServerHub = false;
  bool _serverRemovalCleanupPending = false;
  RhythmMode? _pendingModeAction;
  Timer? _pendingModeActionClearTimer;
  RoomProvider? _pendingModeActionRoomProvider;
  VoidCallback? _pendingModeActionRoomListener;

  /// Sticky flag governing the [ServerDisconnectedScreen]. Set true only when
  /// we've been unable to reach the server *and* have never completed a hello
  /// with it for longer than [_serverConnectGrace] (so we don't flash the
  /// disconnect screen during the normal connect/reconnect handshake — e.g.
  /// app resume or a bridge waking up). Cleared on
  /// [RhythmConnectionState.connected].
  bool _serverLostConnection = false;

  /// How long a cold connection may stay unconnected before we escalate from
  /// the "Setting up…" loader to the full [ServerDisconnectedScreen]. Covers
  /// the transient `connecting`/`reconnecting` blip seen on app resume and on
  /// the first tap into a server hub (the RPi Zero bridge can take a moment
  /// to answer the first hello).
  static const _serverConnectGrace = Duration(seconds: 5);
  Timer? _serverConnectGraceTimer;

  MainNavTab get _currentTab => _tabs[_tabIndex];

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    _loadData();
    AnalyticsService().logScreenView('home');
  }

  @override
  void dispose() {
    _cancelPendingModeActionHandoff();
    _serverConnectGraceTimer?.cancel();
    WidgetsBinding.instance.removeObserver(this);
    _roomPageController.dispose();
    super.dispose();
  }

  /// Arm the grace timer that escalates a stalled cold connection to the
  /// retry screen. Idempotent — repeated calls during rebuilds keep the
  /// single in-flight window rather than restarting it.
  void _startServerConnectGrace() {
    if (_serverConnectGraceTimer != null) return;
    _serverConnectGraceTimer = Timer(_serverConnectGrace, () {
      _serverConnectGraceTimer = null;
      if (!mounted) return;
      setState(() => _serverLostConnection = true);
    });
  }

  void _cancelServerConnectGrace() {
    _serverConnectGraceTimer?.cancel();
    _serverConnectGraceTimer = null;
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state == AppLifecycleState.resumed) {
      _pingServer();
    }
  }

  void _pingServer() {
    try {
      context.read<ServerSyncProvider>().retryActiveServerConnection();
    } catch (e) {
      // Server not available, ignore
    }
  }

  Future<void> _loadData() async {
    try {
      final api = context.read<RhythmApi>();
      final configModel = context.read<ConfigModel>();

      final results = await Future.wait([
        api.getConfigState(),
        api.getCurveData(),
      ]);

      if (mounted) {
        final configState = results[0] as ConfigState;
        final curveData = results[1] as CurveData;

        configModel.updateFromConfigState(configState);

        setState(() {
          _curveData = curveData;
        });
      }
    } catch (e) {
      debugPrint('AppShell: Config/curve load failed: $e');
    }

    // Sync rooms from cloud/hubs — must run even if config load failed
    // (HA addon has no WASM brain, but still needs to fetch rooms from backend)
    try {
      if (mounted) {
        await AppStateRefresh.sync(context,
            options: const SyncOptions(
              rooms: true,
            ));
      }
    } catch (e) {
      debugPrint('AppShell: Sync failed: $e');
    }
  }

  void _handleTabSelected(MainNavTab tab) {
    final newIndex = _tabs.indexOf(tab);
    if (newIndex < 0) return;

    if (newIndex == _tabIndex) {
      // Re-tap of the active tab pops its nested stack to root.
      _navKeys[tab]?.currentState?.popUntil((route) => route.isFirst);
      return;
    }

    setState(() {
      _tabIndex = newIndex;
      _builtTabs.add(tab);
    });
    AnalyticsService().logScreenView(_screenNameFor(tab));
  }

  String _screenNameFor(MainNavTab tab) => switch (tab) {
        MainNavTab.home => 'home',
        MainNavTab.light => 'light',
        MainNavTab.automations => 'automations',
        MainNavTab.devices => 'devices',
        MainNavTab.settings => 'settings',
      };

  /// Intercepts the system back button:
  ///   1. pop the active tab's nested stack if possible,
  ///   2. else fall back to the Home tab,
  ///   3. else allow the OS to exit the app.
  Future<void> _handlePopInvoked(bool didPop) async {
    if (didPop) return;
    final nav = _navKeys[_currentTab]?.currentState;
    if (nav != null && nav.canPop()) {
      nav.pop();
      return;
    }
    if (_tabIndex != 0) {
      setState(() => _tabIndex = 0);
      return;
    }
    await SystemNavigator.pop();
  }

  void _handleServerHubLifecycle({required bool hasServerHub}) {
    final serverRemoved = _hadServerHub && !hasServerHub;
    _hadServerHub = hasServerHub;

    if (!serverRemoved || _serverRemovalCleanupPending) return;
    _serverRemovalCleanupPending = true;

    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      // A removed server hub means the LightBox pairing is gone. Collapse any
      // nested tab stacks and send the user back to Home, where the hardware
      // onboarding gate is shown when there are no rooms.
      for (final navKey in _navKeys.values) {
        navKey.currentState?.popUntil((route) => route.isFirst);
      }
      setState(() {
        _tabIndex = _tabs.indexOf(MainNavTab.home);
        _builtTabs.add(MainNavTab.home);
        _serverLostConnection = false;
      });
      _serverRemovalCleanupPending = false;
    });
  }

  String _modeLabel(RhythmMode mode) {
    return switch (mode) {
      RhythmMode.day => 'Day',
      RhythmMode.sleep => 'Sleep',
    };
  }

  String _defaultTransitionIdForMode(RhythmMode mode) {
    return switch (mode) {
      RhythmMode.day => 'sleep_to_day',
      RhythmMode.sleep => 'day_to_sleep',
    };
  }

  void _showModeActionError(String message) {
    if (!mounted) return;
    ScaffoldMessenger.maybeOf(context)?.showSnackBar(
      SnackBar(content: Text(message)),
    );
  }

  void _cancelPendingModeActionHandoff() {
    _pendingModeActionClearTimer?.cancel();
    _pendingModeActionClearTimer = null;
    final provider = _pendingModeActionRoomProvider;
    final listener = _pendingModeActionRoomListener;
    if (provider != null && listener != null) {
      provider.removeListener(listener);
    }
    _pendingModeActionRoomProvider = null;
    _pendingModeActionRoomListener = null;
  }

  void _clearPendingModeAction(RhythmMode mode) {
    _cancelPendingModeActionHandoff();
    if (!mounted || _pendingModeAction != mode) return;
    setState(() => _pendingModeAction = null);
  }

  void _handoffPendingModeActionToRoomTransition(RhythmMode mode) {
    if (!mounted || _pendingModeAction != mode) return;
    _cancelPendingModeActionHandoff();

    final roomProvider = context.read<RoomProvider>();
    late VoidCallback listener;
    listener = () {
      if (!mounted || _pendingModeAction != mode) {
        _cancelPendingModeActionHandoff();
        return;
      }
      if (roomProvider.anyRoomTransitioning) {
        _clearPendingModeAction(mode);
      }
    };

    _pendingModeActionRoomProvider = roomProvider;
    _pendingModeActionRoomListener = listener;
    roomProvider.addListener(listener);
    _pendingModeActionClearTimer = Timer(
      _modeActionSseHandoffGrace,
      () => _clearPendingModeAction(mode),
    );
    listener();
  }

  /// Flip the global mode from the All Rooms toggle.
  Future<void> _setActiveMode(RhythmMode mode) async {
    if (_pendingModeAction != null) return;
    HapticFeedback.mediumImpact();
    final roomProvider = context.read<RoomProvider>();
    if (roomProvider.anyRoomTransitioning) return;
    final serverSync = context.read<ServerSyncProvider>();
    final currentMode = serverSync.activeMode;
    if (currentMode == mode) return;

    setState(() => _pendingModeAction = mode);
    var handoffToRoomTransition = false;
    try {
      final transitionId =
          currentMode == null ? null : _defaultTransitionIdForMode(mode);
      if (transitionId == null) {
        await serverSync.dispatchSetActiveMode(mode);
        AnalyticsService().logGlobalModeChanged(
          mode.name,
          source: 'all_rooms_toggle',
        );
        return;
      }

      final success = await serverSync.dispatchRunTransition(transitionId);
      if (!success) return;
      handoffToRoomTransition = true;
      AnalyticsService().logGlobalModeChanged(
        mode.name,
        source: 'all_rooms_toggle',
      );
    } finally {
      if (handoffToRoomTransition) {
        _handoffPendingModeActionToRoomTransition(mode);
      } else {
        _clearPendingModeAction(mode);
      }
    }
  }

  Future<void> _confirmReapplyActiveMode(RhythmMode mode) async {
    if (_pendingModeAction != null) return;
    final roomProvider = context.read<RoomProvider>();
    if (roomProvider.anyRoomTransitioning) return;

    final modeLabel = _modeLabel(mode);
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: Text('Set all lights to $modeLabel settings?'),
        content: const Text(
          'This reapplies the current preset for every light.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(false),
            child: const Text('Cancel'),
          ),
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(true),
            child: const Text('Set All'),
          ),
        ],
      ),
    );

    if (confirmed != true || !mounted) return;
    await _reapplyActiveMode(mode);
  }

  Future<void> _reapplyActiveMode(RhythmMode mode) async {
    if (_pendingModeAction != null) return;
    final roomProvider = context.read<RoomProvider>();
    if (roomProvider.anyRoomTransitioning) return;

    final serverSync = context.read<ServerSyncProvider>();
    final modeLabel = _modeLabel(mode);

    if (!serverSync.canDispatchActions) {
      _showModeActionError('Could not reapply $modeLabel settings.');
      return;
    }

    HapticFeedback.mediumImpact();

    setState(() => _pendingModeAction = mode);
    var handoffToRoomTransition = false;
    try {
      final ranTransition = await serverSync
          .dispatchRunTransition(_defaultTransitionIdForMode(mode));
      if (ranTransition) {
        handoffToRoomTransition = true;
        if (mounted) HapticFeedback.heavyImpact();
        return;
      }

      _showModeActionError('Could not reapply $modeLabel settings.');
    } finally {
      if (handoffToRoomTransition) {
        _handoffPendingModeActionToRoomTransition(mode);
      } else {
        _clearPendingModeAction(mode);
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final hasServerHub = context.select<HomeProvider, bool>(
      (homeProvider) => homeProvider.getFirstHubOfType(HubType.server) != null,
    );
    _handleServerHubLifecycle(hasServerHub: hasServerHub);

    // The "Do you have a LightBox?" gate occupies the full Home-tab body and
    // shouldn't show the bottom nav — it's effectively the first onboarding
    // step a fresh user lands on. Mirror the same conditions used inside
    // `_buildHomeTab` so the two stay aligned.
    final hasRooms = context.select<RoomProvider, bool>((r) => r.hasRooms);
    final showingHardwareGate = _currentTab == MainNavTab.home &&
        !hasServerHub &&
        !hasRooms &&
        !HueServiceLocator.isDemoMode;

    // `hasBeenSynced` is the sticky flag (true once a hello has completed,
    // false only on full hub unpair). Reading the non-sticky `synced` here
    // would dip false during transient SSE/poll reconnects — including the
    // reconnect that fires right after any HTTP action against the server
    // (e.g. toggling the Time/Button trigger switches or starting a listen
    // for a button press), evicting the user from the Automations tab back
    // to Home mid-interaction.
    final serverSynced = context.select<ServerSyncProvider, bool>(
      (s) => s.hasBeenSynced,
    );
    final visibleTabs = _computeVisibleTabs(serverSynced: serverSynced);
    final disabledTabs = _computeDisabledTabs(serverSynced: serverSynced);

    // If the current tab depends on a synced server (e.g. Automations) and
    // the server hub was actually unpaired, bounce the user back to Home so
    // they can't sit on a disabled tab's body.
    if (disabledTabs.contains(_currentTab)) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (!mounted) return;
        final synced = context.read<ServerSyncProvider>().hasBeenSynced;
        if (_computeDisabledTabs(serverSynced: synced).contains(_currentTab)) {
          setState(() {
            _tabIndex = 0;
            _builtTabs.add(_tabs[0]);
          });
        }
      });
    }

    return PopScope(
      canPop: false,
      onPopInvokedWithResult: (didPop, _) => _handlePopInvoked(didPop),
      child: ListenableBuilder(
        listenable: VirtualExperienceService.instance,
        builder: (context, _) {
          final isVirtual = VirtualExperienceService.instance.isActive;
          return Scaffold(
            backgroundColor: CelestialColors.backgroundDark,
            body: Stack(
              children: [
                Column(
                  children: [
                    if (isVirtual)
                      VirtualExperienceBanner(
                        onExit: VirtualExperienceService.instance.exit,
                      ),
                    Expanded(
                      // Banner already consumed the status-bar inset; the tab
                      // screens below use SafeArea(top:true) and would otherwise
                      // double-pad.
                      child: MediaQuery.removePadding(
                        context: context,
                        removeTop: isVirtual,
                        child: Stack(
                          fit: StackFit.expand,
                          children: List.generate(
                            _tabs.length,
                            _buildTabSlot,
                          ),
                        ),
                      ),
                    ),
                  ],
                ),
                if (!showingHardwareGate)
                  Positioned(
                    right: 14,
                    bottom: 14,
                    child: _FloatingSunButton(
                      onTap: () => SunPositionScreen.show(context),
                    ),
                  ),
              ],
            ),
            bottomNavigationBar: showingHardwareGate
                ? null
                : _buildBottomNav(
                    visibleTabs: visibleTabs,
                    disabledTabs: disabledTabs,
                  ),
          );
        },
      ),
    );
  }

  /// Bottom-nav tab visibility — all five tabs are always rendered so the
  /// navbar shape stays stable. Tabs whose dependencies aren't met get
  /// disabled via [_computeDisabledTabs] instead of being hidden.
  List<MainNavTab> _computeVisibleTabs({required bool serverSynced}) {
    return const [
      MainNavTab.home,
      MainNavTab.light,
      MainNavTab.automations,
      MainNavTab.devices,
      MainNavTab.settings,
    ];
  }

  /// Tabs to grey-out and reject taps on. Automations is disabled until the
  /// Rhythm server has completed at least one hello — the orbital editor
  /// needs real profile data to render meaningfully. Uses the sticky
  /// `hasBeenSynced` signal so the disabled state doesn't flicker on
  /// transient reconnects.
  Set<MainNavTab> _computeDisabledTabs({required bool serverSynced}) {
    return {
      if (!serverSynced) MainNavTab.automations,
    };
  }

  Widget _buildTabSlot(int index) {
    final tab = _tabs[index];
    final isActive = index == _tabIndex;
    return Offstage(
      offstage: !isActive,
      child: TickerMode(
        enabled: isActive,
        child: _builtTabs.contains(tab)
            ? _TabNavigator(
                navigatorKey: _navKeys[tab]!,
                builder: (_) => _buildTabRoot(tab),
              )
            : const SizedBox.shrink(),
      ),
    );
  }

  Widget _buildTabRoot(MainNavTab tab) {
    return switch (tab) {
      MainNavTab.home => _buildHomeTab(),
      MainNavTab.light => const LightScreen(),
      MainNavTab.automations => _buildAutomationsTab(),
      MainNavTab.devices =>
        const LightsDevicesDetailScreen(showBackButton: false),
      MainNavTab.settings => const SettingsScreen(),
    };
  }

  Widget _buildBottomNav({
    required List<MainNavTab> visibleTabs,
    required Set<MainNavTab> disabledTabs,
  }) {
    return MainBottomNav(
      currentBodyTab: _currentTab,
      onTabSelected: _handleTabSelected,
      tabs: visibleTabs,
      disabledTabs: disabledTabs,
    );
  }

  /// Automations tab — a list of automations, each tapping into its own detail
  /// screen (the orbital schedule, button binding, and per-mode room behavior).
  Widget _buildAutomationsTab() {
    return const AutomationsScreen();
  }

  /// Resolves the body for the Home tab through the existing
  /// connection/rooms state machine.
  Widget _buildHomeTab() {
    return Consumer3<RoomProvider, HomeProvider, ServerSyncProvider>(
      builder: (context, roomProvider, homeProvider, serverSync, child) {
        final serverHub = homeProvider.getFirstHubOfType(HubType.server);
        final state = serverSync.connectionState;

        // Govern the disconnect screen without flashing it during the normal
        // connect handshake:
        //   • `connected`              → clear the flag + grace timer, show
        //     the room grid.
        //   • not connected, never synced (`connecting`/`reconnecting`/
        //     `disconnected` from a cold start) → arm the grace timer instead
        //     of flagging immediately. We keep showing the "Setting up…"
        //     loader (no rooms) or the cached room grid (resume) until the
        //     timer fires; only then do we surface the retry screen. This
        //     covers the resume blip and a bridge taking a moment to answer
        //     its first hello.
        //   • not connected, already synced (transient blip after a save /
        //     pull-to-refresh) → leave the flag and timer alone so the grid
        //     stays put.
        if (serverHub == null || state == RhythmConnectionState.connected) {
          _serverLostConnection = false;
          _cancelServerConnectGrace();
        } else if (!serverSync.hasBeenSynced && !_serverLostConnection) {
          _startServerConnectGrace();
        }

        if (serverHub != null) {
          if (serverSync.hasHomeEntryRefreshGate) {
            return _buildHomeEntryState(
              serverSync: serverSync,
              serverHub: serverHub,
              homeName: homeProvider.currentHome?.name,
            );
          }

          if (_serverLostConnection) {
            return ServerDisconnectedScreen(serverHub: serverHub);
          }

          if (state == RhythmConnectionState.connected) {
            if (roomProvider.hasRooms) {
              return _buildRoomGrid(roomProvider);
            }
            return const HubPickerScreen();
          }

          // First connection attempt.
          if (!roomProvider.hasRooms) {
            return _buildServerConnectingState();
          }

          return _buildRoomGrid(roomProvider);
        }

        // No server hub paired.
        if (!roomProvider.hasRooms) {
          if (HueServiceLocator.isDemoMode) {
            return _buildServerConnectingState();
          }
          return _buildNoRoomsLayout(ConnectHubMode.rhythmServer);
        }

        return _buildRoomGrid(roomProvider);
      },
    );
  }

  /// The main room constellation grid (Home tab body content).
  Widget _buildRoomGrid(RoomProvider roomProvider) {
    final serverSync = context.watch<ServerSyncProvider>();
    final enabledRooms = roomProvider.enabledRooms;
    final visibleRooms =
        enabledRooms.where(showsInAllRooms).toList(growable: false);

    // Reconcile page assignments after frame to avoid notifying during build.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      context.read<RoomPageProvider>().reconcileRooms(visibleRooms);
    });

    return Consumer<ConfigModel>(
      builder: (context, configModel, _) {
        return AllRoomsScreen(
          rooms: visibleRooms,
          globalConfig: configModel.config,
          curveData: _curveData,
          pageController: _roomPageController,
          activeMode: serverSync.activeMode,
          pendingMode: _pendingModeAction ??
              (roomProvider.anyRoomTransitioning
                  ? serverSync.activeMode
                  : null),
          onModeSelected: _setActiveMode,
          onActiveModeDoubleTap: _confirmReapplyActiveMode,
          onHomeChooserTap: () => ConnectHubScreen.show(
            context,
            mode: ConnectHubMode.rhythmServer,
          ),
        );
      },
    );
  }

  /// Explicit Home-entry/login state. This blocks cached rooms until the hub
  /// has delivered a fresh authoritative hello for the selected Home.
  Widget _buildHomeEntryState({
    required ServerSyncProvider serverSync,
    required Hub serverHub,
    String? homeName,
  }) {
    final displayHome =
        serverSync.homeEntryRefreshHomeName ?? homeName ?? 'Home';
    final error = serverSync.homeEntryRefreshError;
    final hasTunnel = serverHub.remoteEndpoint != null;
    final routeText = hasTunnel
        ? 'Selecting local network or secure tunnel'
        : 'Connecting on your local network';
    final description = error == null
        ? [
            'Connecting to ${serverHub.name}',
            routeText,
            'Syncing latest rooms and settings',
          ].join('\n')
        : 'We could not get fresh room data from ${serverHub.name}.';

    return ServerDisconnectedScreen(
      serverHub: serverHub,
      title: error ?? 'Entering $displayHome',
      addressLabel: serverHub.name,
      description: description,
      statusLabel: error == null ? 'Connecting\u2026' : 'Refresh failed',
      statusActive: error == null,
      retryLabel: 'Retry',
      onRetry: () {
        unawaited(serverSync.refreshForHomeEntry(homeName: displayHome));
      },
    );
  }

  /// Server hub is connected/connecting but has no rooms yet (Hue discovery in progress).
  Widget _buildServerConnectingState() {
    return SafeArea(
      bottom: false,
      child: Center(
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 40),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              _PulsingIcon(
                icon: Icons.hub,
                color: CelestialColors.accentBlue,
              ),
              const SizedBox(height: 24),
              Text(
                'Setting up...',
                style: TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 22,
                  fontWeight: FontWeight.w600,
                ),
              ),
              const SizedBox(height: 12),
              Text(
                'Connecting to your lights',
                textAlign: TextAlign.center,
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.7),
                  fontSize: 15,
                  height: 1.5,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }

  /// Onboarding gate shown when no rooms are synced and no server hub paired.
  Widget _buildNoRoomsLayout(ConnectHubMode mode) {
    return SafeArea(
      bottom: false,
      child: HardwareOnboardingGate(mode: mode),
    );
  }
}

/// Wraps a single tab in its own [Navigator] so route pushes happen inside
/// the body slot, leaving the bottom navigation bar untouched.
class _TabNavigator extends StatelessWidget {
  final GlobalKey<NavigatorState> navigatorKey;
  final WidgetBuilder builder;

  const _TabNavigator({
    required this.navigatorKey,
    required this.builder,
  });

  @override
  Widget build(BuildContext context) {
    return Navigator(
      key: navigatorKey,
      onGenerateRoute: (settings) =>
          MaterialPageRoute(builder: builder, settings: settings),
    );
  }
}

/// Pulsing icon for the server-connecting state.
class _PulsingIcon extends StatefulWidget {
  final IconData icon;
  final Color color;
  const _PulsingIcon({required this.icon, required this.color});

  @override
  State<_PulsingIcon> createState() => _PulsingIconState();
}

class _PulsingIconState extends State<_PulsingIcon>
    with SingleTickerProviderStateMixin {
  late final AnimationController _controller;

  @override
  void initState() {
    super.initState();
    _controller = AnimationController(
      vsync: this,
      duration: const Duration(seconds: 2),
    )..repeat(reverse: true);
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: _controller,
      builder: (context, child) {
        final opacity = 0.3 + 0.7 * _controller.value;
        return Icon(
          widget.icon,
          size: 64,
          color: widget.color.withValues(alpha: opacity),
        );
      },
    );
  }
}

/// Small floating amber sun chip pinned bottom-right above the nav bar.
/// Entry point for [SunPositionScreen] — the celestial visualization no
/// longer has its own tab, so it lives here as a discoverable hover.
class _FloatingSunButton extends StatelessWidget {
  const _FloatingSunButton({required this.onTap});

  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    const warm = Color(0xFFFFB74D);
    const deep = Color(0xFFE6892E);
    return Semantics(
      button: true,
      label: 'Open sun position',
      child: Material(
        color: Colors.transparent,
        shape: const CircleBorder(),
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          onTap: () {
            HapticFeedback.selectionClick();
            onTap();
          },
          child: Container(
            width: 44,
            height: 44,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              gradient: const LinearGradient(
                begin: Alignment.topLeft,
                end: Alignment.bottomRight,
                colors: [warm, deep],
              ),
              boxShadow: [
                BoxShadow(
                  color: warm.withValues(alpha: 0.45),
                  blurRadius: 16,
                  spreadRadius: -2,
                ),
                BoxShadow(
                  color: Colors.black.withValues(alpha: 0.30),
                  blurRadius: 8,
                  offset: const Offset(0, 2),
                ),
              ],
              border: Border.all(
                color: Colors.white.withValues(alpha: 0.18),
              ),
            ),
            child: const Icon(
              Icons.wb_sunny_rounded,
              color: Colors.white,
              size: 22,
            ),
          ),
        ),
      ),
    );
  }
}
