import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show RhythmConnectionState, RhythmMode;
import 'models/config_model.dart';
import 'backend/auth/auth_user.dart';
import 'providers/room_provider.dart';
import 'providers/home_provider.dart';
import 'providers/hub_connection_provider.dart';
import 'providers/room_page_provider.dart';
import 'providers/server_sync_provider.dart';
import 'screens/all_rooms_screen.dart';
import 'screens/triage_screen.dart';
import 'screens/server_disconnected_screen.dart';
import 'screens/settings/automations_screen.dart';
import 'screens/settings/dialogs/sign_in_modal.dart';
import 'screens/settings/light_screen.dart';
import 'screens/settings/settings_screen.dart';
import 'config/feature_flags.dart';
import 'config/platform_capabilities.dart';
import 'main.dart';
import 'services/account_session_service.dart';
import 'services/analytics_service.dart';
import 'services/app_state_refresh.dart';
import 'services/auth_service.dart';
import 'services/hue/hue_service_locator.dart';
import 'services/virtual_experience_service.dart';
import 'utils/room_visibility.dart';
import 'widgets/connect_hub_screen.dart';
import 'widgets/disabled_mode_banner.dart';
import 'widgets/hardware_gate_screen.dart';
import 'widgets/hub_connection_loading_screen.dart';
import 'widgets/hub_picker_screen.dart';
import 'widgets/main_bottom_nav.dart';
import 'widgets/report_bug_flow.dart';
import 'widgets/solar_orbit.dart';
import 'widgets/virtual_experience_banner.dart';

/// Main app shell.
///
/// Persistent top-level destinations with an [IndexedStack] body. Each
/// destination owns its own [Navigator] so deeper pushes stay inside its body
/// slot.
class AppShell extends StatefulWidget {
  const AppShell({super.key});

  @override
  State<AppShell> createState() => _AppShellState();
}

class _AppShellState extends State<AppShell> with WidgetsBindingObserver {
  static const _modeActionSseHandoffGrace = Duration(seconds: 3);

  static const _rhythmAdaptiveTabs = [
    MainNavTab.home,
    MainNavTab.automations,
    MainNavTab.lighting,
    MainNavTab.settings,
  ];

  int _tabIndex = 0;
  // Keyed by tab identity (not index) so swapping tab sets doesn't strand
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
  bool _accountActionInProgress = false;
  bool _activeTabAtRoot = true;
  bool _routeStateSyncQueued = false;

  /// Sticky flag governing the [ServerDisconnectedScreen]. Set true only when
  /// we've been unable to reach the server *and* have never completed a hello
  /// with it for longer than [_serverConnectGrace] (so we don't flash the
  /// disconnect screen during the normal connect/reconnect handshake — e.g.
  /// app resume or a bridge waking up). Cleared once the selected server has
  /// connected far enough for the shell to leave the retry screen.
  bool _serverLostConnection = false;

  /// How long a cold connection may stay unconnected before we escalate from
  /// the "Setting up…" loader to the full [ServerDisconnectedScreen]. Covers
  /// the transient `connecting`/`reconnecting` blip seen on app resume and on
  /// the first tap into a server hub (the RPi Zero bridge can take a moment
  /// to answer the first hello).
  static const _serverConnectGrace = Duration(seconds: 5);
  Timer? _serverConnectGraceTimer;

  List<MainNavTab> get _activeTabs => _rhythmAdaptiveTabs;

  MainNavTab get _currentTab {
    final tabs = _activeTabs;
    if (_tabIndex < 0 || _tabIndex >= tabs.length) return tabs.first;
    return tabs[_tabIndex];
  }

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
      _refreshActiveHomeOnResume();
    }
  }

  void _refreshActiveHomeOnResume() {
    try {
      final homeProvider = context.read<HomeProvider>();
      final serverHub = homeProvider.getFirstHubOfType(HubType.server);
      if (serverHub == null) return;

      final serverSync = context.read<ServerSyncProvider>();
      if (!serverSync.hasBeenSynced || serverSync.connectedServerHub == null) {
        return;
      }

      unawaited(
        serverSync
            .retryActiveServerConnection(
          authoritative: true,
        )
            .catchError((Object error, StackTrace stackTrace) {
          debugPrint('AppShell: Resume server refresh failed: $error');
          debugPrint('$stackTrace');
        }),
      );
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
    final tabs = _activeTabs;
    final newIndex = tabs.indexOf(tab);
    if (newIndex < 0) return;

    if (newIndex == _tabIndex) {
      // Re-tap of the active tab pops its nested stack to root.
      _navKeys[tab]?.currentState?.popUntil((route) => route.isFirst);
      _scheduleActiveTabRouteSync();
      return;
    }

    setState(() {
      _tabIndex = newIndex;
      _builtTabs.add(tab);
    });
    _scheduleActiveTabRouteSync();
    AnalyticsService().logScreenView(_screenNameFor(tab));
  }

  void _scheduleActiveTabRouteSync() {
    if (_routeStateSyncQueued) return;
    _routeStateSyncQueued = true;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _routeStateSyncQueued = false;
      if (!mounted) return;
      final navigator = _navKeys[_currentTab]?.currentState;
      final activeTabAtRoot = !(navigator?.canPop() ?? false);
      if (_activeTabAtRoot == activeTabAtRoot) return;
      setState(() => _activeTabAtRoot = activeTabAtRoot);
    });
  }

  String _screenNameFor(MainNavTab tab) => switch (tab) {
        MainNavTab.home => 'home',
        MainNavTab.automations => 'automations',
        MainNavTab.lighting => 'light',
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
        _tabIndex = _rhythmAdaptiveTabs.indexOf(MainNavTab.home);
        _builtTabs.add(MainNavTab.home);
        _serverLostConnection = false;
      });
      _serverRemovalCleanupPending = false;
    });
  }

  void _showPreHomeSignIn() {
    SignInModal.show(context);
  }

  Future<void> _showReportBug() async {
    final serverHub =
        context.read<HomeProvider>().getFirstHubOfType(HubType.server);
    await showReportBugFlow(context, serverHub: serverHub);
  }

  /// Leave the Virtual Experience: clear the seeded demo state and land back
  /// at the start of the hardware onboarding funnel. Runs the same local
  /// reset as logout — [AccountSessionService.resetLocalSessionState] also
  /// exits [VirtualExperienceService] — but never touches a real session
  /// (the Virtual Experience is only reachable signed out).
  Future<void> _exitVirtualExperience() async {
    if (_accountActionInProgress) return;
    setState(() => _accountActionInProgress = true);

    AnalyticsService().logEvent('virtual_experience_exited');
    try {
      await AccountSessionService.instance.resetLocalSessionState(
        serverSyncProvider: context.read<ServerSyncProvider>(),
        homeProvider: context.read<HomeProvider>(),
        hubProvider: context.read<HubConnectionProvider>(),
        roomProvider: context.read<RoomProvider>(),
        debugLabel: 'Exit Virtual Experience',
      );
    } catch (error, stackTrace) {
      debugPrint('Exit Virtual Experience: Failed: $error');
      debugPrint('$stackTrace');
    } finally {
      if (mounted) {
        setState(() => _accountActionInProgress = false);
      }
    }

    if (!mounted) return;
    Navigator.of(context, rootNavigator: true)
        .popUntil((route) => route.isFirst);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      AuthGate.resetToOnboarding();
    });
  }

  Future<void> _logOutFromPreHome() async {
    if (_accountActionInProgress) return;

    final confirmed = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: const Text('Log out?'),
        content: const Text(
          'This clears local app state on this device. You can sign back in later.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(false),
            child: const Text('Cancel'),
          ),
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(true),
            child: const Text(
              'Log Out',
              style: TextStyle(color: Color(0xFFFFC857)),
            ),
          ),
        ],
      ),
    );

    if (confirmed != true || !mounted) return;

    setState(() => _accountActionInProgress = true);
    try {
      await AccountSessionService.instance.logOutAndReset(
        serverSyncProvider: context.read<ServerSyncProvider>(),
        homeProvider: context.read<HomeProvider>(),
        hubProvider: context.read<HubConnectionProvider>(),
        roomProvider: context.read<RoomProvider>(),
      );
    } catch (error, stackTrace) {
      debugPrint('Pre-home Log Out: Failed: $error');
      debugPrint('$stackTrace');
      if (mounted) {
        ScaffoldMessenger.maybeOf(context)?.showSnackBar(
          SnackBar(content: Text('Could not log out: $error')),
        );
      }
      return;
    } finally {
      if (mounted) {
        setState(() => _accountActionInProgress = false);
      }
    }

    if (!mounted) return;
    Navigator.of(context, rootNavigator: true)
        .popUntil((route) => route.isFirst);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      AuthGate.resetToOnboarding();
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
    final activeServerHub = context.select<HomeProvider, Hub?>(
      (homeProvider) => homeProvider.getFirstHubOfType(HubType.server),
    );
    final hasServerHub = activeServerHub != null;
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

    // The "Entering Home…" handshake takes over the full Home-tab body. Like
    // the hardware gate, it's a blocking full-screen state — drop the bottom
    // nav so the server connection screen reads as one surface.
    final enteringHome = hasServerHub &&
        context.select<ServerSyncProvider, bool>(
          (s) => s.hasHomeEntryRefreshGate,
        );
    final hideChrome = showingHardwareGate || enteringHome;
    final showBottomFloatingChrome =
        _currentTab == MainNavTab.home && !hideChrome && _activeTabAtRoot;
    final hasCurrentHome = context.select<HomeProvider, bool>(
      (homeProvider) => homeProvider.currentHome != null,
    );
    // The hardware gate renders its own account control inside its header
    // (see `_buildNoRoomsLayout`), so exclude that case here to avoid both a
    // duplicate and the floating overlay clashing with sub-screen titles.
    final accountControlCandidate = !showingHardwareGate &&
        (hideChrome || (!hasCurrentHome && !hasServerHub));

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
            _builtTabs.add(_activeTabs.first);
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
          final showPreHomeAccountControl =
              !isVirtual && accountControlCandidate;
          return AnimatedSwitcher(
            duration: const Duration(milliseconds: 650),
            switchInCurve: Curves.easeOutBack,
            switchOutCurve: Curves.easeInCubic,
            child: Scaffold(
              backgroundColor: CelestialColors.backgroundDark,
              body: Stack(
                children: [
                  // Disabled-mode banner shows only when the server is reachable
                  // but autonomous light control is off. A Selector keeps the
                  // tab stack from rebuilding on every ServerSync notification.
                  Selector<ServerSyncProvider, bool>(
                    selector: (_, sync) =>
                        !isVirtual &&
                        sync.synced &&
                        sync.roomsReadyForDisplay &&
                        !sync.lightBreakerEnabled,
                    builder: (context, showDisabledBanner, _) {
                      return Column(
                        children: [
                          if (isVirtual)
                            VirtualExperienceBanner(
                              onExit: () => unawaited(_exitVirtualExperience()),
                            ),
                          if (showDisabledBanner)
                            DisabledModeBanner(
                              onEnable: () => unawaited(
                                context
                                    .read<ServerSyncProvider>()
                                    .setLightBreakerEnabled(true),
                              ),
                            ),
                          Expanded(
                            // The top banner (if any) already consumed the
                            // status-bar inset; the tab screens below use
                            // SafeArea(top:true) and would otherwise double-pad.
                            child: MediaQuery.removePadding(
                              context: context,
                              removeTop: isVirtual || showDisabledBanner,
                              child: Stack(
                                fit: StackFit.expand,
                                children: List.generate(
                                  _activeTabs.length,
                                  _buildTabSlot,
                                ),
                              ),
                            ),
                          ),
                        ],
                      );
                    },
                  ),
                  if (enteringHome)
                    Positioned.fill(
                      child: Consumer2<HomeProvider, ServerSyncProvider>(
                        builder: (context, homeProvider, serverSync, _) {
                          final serverHub =
                              homeProvider.getFirstHubOfType(HubType.server);
                          if (serverHub == null) {
                            return const SizedBox.shrink();
                          }
                          return _buildHomeEntryState(
                            serverSync: serverSync,
                            serverHub: serverHub,
                            homeName: homeProvider.currentHome?.name,
                          );
                        },
                      ),
                    ),
                  if (showBottomFloatingChrome)
                    Positioned(
                      left: 18,
                      bottom: MediaQuery.of(context).padding.bottom + 20,
                      child: _FloatingDevicesButton(
                        onTap: () => TriageScreen.show(context),
                        badgeCount: context.select<ServerSyncProvider, int>(
                          (s) => s.triagePendingCount > 0
                              ? s.triagePendingCount
                              : s.hubConfiguredConflicts.length,
                        ),
                      ),
                    ),
                  if (showPreHomeAccountControl)
                    Positioned(
                      top: MediaQuery.of(context).padding.top + 8,
                      right: 18,
                      child: _PreHomeAccountButton(
                        isBusy: _accountActionInProgress,
                        onSignIn: _showPreHomeSignIn,
                        onLogOut: _logOutFromPreHome,
                      ),
                    ),
                  // No bottom nav bar — navigation fans out of a floating gear
                  // button in the bottom-right corner.
                  if (showBottomFloatingChrome)
                    Positioned.fill(
                      child: _NavFanButton(
                        currentTab: _currentTab,
                        tabs: visibleTabs,
                        disabledTabs: disabledTabs,
                        onSelected: _handleTabSelected,
                        onReportBug: () => unawaited(_showReportBug()),
                      ),
                    ),
                ],
              ),
            ),
          );
        },
      ),
    );
  }

  /// Top-level destination visibility. Destinations whose dependencies aren't
  /// met get disabled via [_computeDisabledTabs] instead of being hidden.
  List<MainNavTab> _computeVisibleTabs({required bool serverSynced}) {
    return _activeTabs;
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
    final tab = _activeTabs[index];
    final isActive = index == _tabIndex;
    return Offstage(
      offstage: !isActive,
      child: TickerMode(
        enabled: isActive,
        child: _builtTabs.contains(tab)
            ? _TabNavigator(
                navigatorKey: _navKeys[tab]!,
                builder: (_) => _buildTabRoot(tab),
                onRouteStackChanged: _scheduleActiveTabRouteSync,
              )
            : const SizedBox.shrink(),
      ),
    );
  }

  Widget _buildTabRoot(MainNavTab tab) {
    return switch (tab) {
      MainNavTab.home => _buildHomeTab(),
      MainNavTab.automations => _buildAutomationsTab(),
      MainNavTab.lighting => LightScreen(onClose: _goHome),
      MainNavTab.settings => SettingsScreen(onClose: _goHome),
    };
  }

  /// Return to the Home tab — the ✕ on the fanned-out menus.
  void _goHome() => _handleTabSelected(MainNavTab.home);

  /// Automations tab — a list of automations, each tapping into its own detail
  /// screen (the orbital schedule, button binding, and per-mode room behavior).
  Widget _buildAutomationsTab() {
    return AutomationsScreen(onClose: _goHome);
  }

  /// Resolves the body for the Home tab through the existing
  /// connection/rooms state machine.
  Widget _buildHomeTab() {
    return Consumer3<RoomProvider, HomeProvider, ServerSyncProvider>(
      builder: (context, roomProvider, homeProvider, serverSync, child) {
        final serverHub = homeProvider.getFirstHubOfType(HubType.server);
        final state = serverSync.connectionState;
        final roomsReady = serverSync.roomsReadyForDisplay;

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
        } else if ((!serverSync.hasBeenSynced ||
                serverSync.isRoomReadinessRefreshPending) &&
            !_serverLostConnection) {
          _startServerConnectGrace();
        }

        if (serverHub != null) {
          if (_serverLostConnection) {
            return ServerDisconnectedScreen(
              serverHub: serverHub,
              showConnectionLoading: true,
              onChooseHome: () => ConnectHubScreen.show(
                context,
                mode: ConnectHubMode.rhythmServer,
              ),
            );
          }

          if (!serverSync.hasBeenSynced || !roomsReady) {
            return _buildServerConnectingState();
          }

          if (state == RhythmConnectionState.connected) {
            if (roomProvider.hasRooms) {
              return _buildRoomGrid(roomProvider);
            }
            return HubPickerScreen(
              onChooseHome: () => ConnectHubScreen.show(
                context,
                mode: ConnectHubMode.rhythmServer,
              ),
            );
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

    void chooseHome() {
      serverSync.cancelHomeEntryRefresh();
      ConnectHubScreen.show(context, mode: ConnectHubMode.rhythmServer);
    }

    if (error == null) {
      return HubConnectionLoadingScreen(onChooseHome: chooseHome);
    }

    return ServerDisconnectedScreen(
      serverHub: serverHub,
      title: 'Server Unreachable',
      onRetry: () async {
        await serverSync.refreshForHomeEntry(
          homeName: displayHome,
          allowWifiFastPath: false,
        );
      },
      // Escape hatch so a hung handshake never traps the user: drop the gate
      // and open the Home chooser.
      onChooseHome: chooseHome,
    );
  }

  /// Server hub is connected/connecting but has no rooms yet (Hue discovery in progress).
  Widget _buildServerConnectingState() {
    return HubConnectionLoadingScreen(
      onChooseHome: () => ConnectHubScreen.show(
        context,
        mode: ConnectHubMode.rhythmServer,
      ),
    );
  }

  /// Onboarding gate shown when no rooms are synced and no server hub paired.
  Widget _buildNoRoomsLayout(ConnectHubMode mode) {
    return SafeArea(
      bottom: false,
      child: HardwareOnboardingGate(
        mode: mode,
        // Anchored into the gate's own header rather than floated from the
        // shell, so it never overlaps a sub-screen's title.
        accountControl: _PreHomeAccountButton(
          isBusy: _accountActionInProgress,
          onSignIn: _showPreHomeSignIn,
          onLogOut: _logOutFromPreHome,
        ),
      ),
    );
  }
}

/// Wraps a single tab in its own [Navigator] so route pushes happen inside
/// the body slot, leaving the bottom navigation bar untouched.
class _TabNavigator extends StatelessWidget {
  final GlobalKey<NavigatorState> navigatorKey;
  final WidgetBuilder builder;
  final VoidCallback onRouteStackChanged;

  const _TabNavigator({
    required this.navigatorKey,
    required this.builder,
    required this.onRouteStackChanged,
  });

  @override
  Widget build(BuildContext context) {
    return Navigator(
      key: navigatorKey,
      observers: [_TabRouteObserver(onRouteStackChanged)],
      onGenerateRoute: (settings) =>
          MaterialPageRoute(builder: builder, settings: settings),
    );
  }
}

class _TabRouteObserver extends NavigatorObserver {
  _TabRouteObserver(this.onChanged);

  final VoidCallback onChanged;

  @override
  void didPush(Route<dynamic> route, Route<dynamic>? previousRoute) {
    super.didPush(route, previousRoute);
    onChanged();
  }

  @override
  void didPop(Route<dynamic> route, Route<dynamic>? previousRoute) {
    super.didPop(route, previousRoute);
    onChanged();
  }

  @override
  void didRemove(Route<dynamic> route, Route<dynamic>? previousRoute) {
    super.didRemove(route, previousRoute);
    onChanged();
  }

  @override
  void didReplace({Route<dynamic>? newRoute, Route<dynamic>? oldRoute}) {
    super.didReplace(newRoute: newRoute, oldRoute: oldRoute);
    onChanged();
  }
}

class _PreHomeAccountButton extends StatelessWidget {
  final bool isBusy;
  final VoidCallback onSignIn;
  final VoidCallback onLogOut;

  const _PreHomeAccountButton({
    required this.isBusy,
    required this.onSignIn,
    required this.onLogOut,
  });

  @override
  Widget build(BuildContext context) {
    final caps = context.read<PlatformCapabilities>();
    if (!caps.hasAccounts || !FeatureFlags.auxSignIn) {
      return const SizedBox.shrink();
    }

    return StreamBuilder<AuthUser?>(
      stream: AuthService().authStateChanges,
      initialData: AuthService().currentUser,
      builder: (context, snapshot) {
        final user = snapshot.data;
        final signedIn = user != null && !user.isAnonymous;
        final label = signedIn ? 'Log out' : 'Sign in';
        // Amber for the signed-in "Log out", celestial blue for "Sign in" —
        // matching the warm/cool accent split used across the gate.
        final accent =
            signedIn ? const Color(0xFFFFC857) : CelestialColors.accentBlue;
        final onTap = isBusy ? null : (signedIn ? onLogOut : onSignIn);

        if (!signedIn) {
          const foreground = Color(0xFF06131F);
          return Container(
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(999),
              boxShadow: [
                BoxShadow(
                  color: accent.withValues(alpha: isBusy ? 0.14 : 0.32),
                  blurRadius: 18,
                  spreadRadius: -2,
                  offset: const Offset(0, 8),
                ),
              ],
            ),
            child: Material(
              color: accent.withValues(alpha: isBusy ? 0.58 : 1.0),
              borderRadius: BorderRadius.circular(999),
              child: InkWell(
                onTap: onTap,
                borderRadius: BorderRadius.circular(999),
                child: Padding(
                  padding:
                      const EdgeInsets.symmetric(horizontal: 14, vertical: 9),
                  child: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      if (isBusy)
                        const SizedBox(
                          width: 14,
                          height: 14,
                          child: CircularProgressIndicator(
                            strokeWidth: 1.8,
                            valueColor:
                                AlwaysStoppedAnimation<Color>(foreground),
                          ),
                        )
                      else
                        const Icon(
                          Icons.login_rounded,
                          size: 16,
                          color: foreground,
                        ),
                      const SizedBox(width: 8),
                      Text(
                        label,
                        style: TextStyle(
                          color: foreground.withValues(
                            alpha: isBusy ? 0.64 : 0.96,
                          ),
                          fontSize: 14,
                          fontWeight: FontWeight.w800,
                          letterSpacing: 0,
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
          );
        }

        // A quiet ghost action, not a floating chip: a small glowing dot +
        // label + trailing glyph, echoing the secondary-link language already
        // used on this screen (`_VirtualExperienceLink`, `_AlreadyHaveOneLink`)
        // so it reads as part of the celestial composition rather than a
        // UI panel pasted on top of it.
        return Material(
          color: Colors.transparent,
          child: InkWell(
            onTap: onTap,
            borderRadius: BorderRadius.circular(12),
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  if (isBusy)
                    SizedBox(
                      width: 12,
                      height: 12,
                      child: CircularProgressIndicator(
                        strokeWidth: 1.6,
                        valueColor: AlwaysStoppedAnimation<Color>(
                          accent.withValues(alpha: 0.7),
                        ),
                      ),
                    )
                  else
                    Container(
                      width: 6,
                      height: 6,
                      decoration: BoxDecoration(
                        shape: BoxShape.circle,
                        color: accent,
                        boxShadow: [
                          BoxShadow(
                            color: accent.withValues(alpha: 0.55),
                            blurRadius: 6,
                            spreadRadius: 0.5,
                          ),
                        ],
                      ),
                    ),
                  const SizedBox(width: 9),
                  Text(
                    label,
                    style: TextStyle(
                      color: CelestialColors.textPrimary.withValues(
                        alpha: isBusy ? 0.5 : 0.9,
                      ),
                      fontSize: 13,
                      fontWeight: FontWeight.w500,
                      letterSpacing: 0.4,
                    ),
                  ),
                  const SizedBox(width: 5),
                  Icon(
                    signedIn
                        ? Icons.logout_rounded
                        : Icons.arrow_forward_rounded,
                    size: 14,
                    color: CelestialColors.textPrimary.withValues(
                      alpha: isBusy ? 0.4 : 0.6,
                    ),
                  ),
                ],
              ),
            ),
          ),
        );
      },
    );
  }
}

/// Floating gear button pinned bottom-right that fans out the navigation
/// destinations (Home · Schedule · Lighting · Settings). Replaces the bottom
/// nav bar: the gear is always visible; tapping it reveals the destinations
/// stacked above it with a staggered reveal, and a scrim dismisses on an
/// outside tap.
class _NavFanButton extends StatefulWidget {
  const _NavFanButton({
    required this.currentTab,
    required this.tabs,
    required this.disabledTabs,
    required this.onSelected,
    required this.onReportBug,
  });

  final MainNavTab currentTab;
  final List<MainNavTab> tabs;
  final Set<MainNavTab> disabledTabs;
  final ValueChanged<MainNavTab> onSelected;
  final VoidCallback onReportBug;

  @override
  State<_NavFanButton> createState() => _NavFanButtonState();
}

class _NavFanButtonState extends State<_NavFanButton>
    with SingleTickerProviderStateMixin {
  late final AnimationController _controller = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 300),
  );
  bool _open = false;

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  void _toggle() {
    HapticFeedback.selectionClick();
    setState(() => _open = !_open);
    _open ? _controller.forward() : _controller.reverse();
  }

  void _close() {
    if (!_open) return;
    setState(() => _open = false);
    _controller.reverse();
  }

  void _select(MainNavTab tab) {
    if (widget.disabledTabs.contains(tab)) return;
    _close();
    widget.onSelected(tab);
  }

  void _reportBug() {
    _close();
    widget.onReportBug();
  }

  /// (outline icon, filled icon, label, accent) per destination.
  static (IconData, IconData, String, Color) _meta(MainNavTab tab) =>
      switch (tab) {
        MainNavTab.home => (
            Icons.home_outlined,
            Icons.home_rounded,
            'Home',
            Color(0xFF00BCD4),
          ),
        MainNavTab.automations => (
            Icons.bolt_outlined,
            Icons.bolt,
            'Schedule',
            Color(0xFFF2A93B),
          ),
        MainNavTab.lighting => (
            Icons.light_mode_outlined,
            Icons.light_mode_rounded,
            'Lighting',
            Color(0xFFFFB74D),
          ),
        MainNavTab.settings => (
            Icons.settings_outlined,
            Icons.settings_rounded,
            'Settings',
            CelestialColors.accentBlue,
          ),
      };

  @override
  Widget build(BuildContext context) {
    // Home is the base surface — you return to it via the ✕ on each menu, so it
    // isn't one of the fanned destinations.
    final tabs =
        widget.tabs.where((t) => t != MainNavTab.home).toList(growable: false);
    final itemCount = tabs.length + 1;
    final fanItems = <Widget>[
      _buildReportBugFanItem(itemCount - 1),
      for (var i = 0; i < tabs.length; i++)
        _buildFanItem(tabs[i], itemCount - 2 - i),
    ];

    return Stack(
      children: [
        if (_open)
          Positioned.fill(
            child: GestureDetector(
              behavior: HitTestBehavior.opaque,
              onTap: _close,
              child: FadeTransition(
                opacity: _controller,
                child: DecoratedBox(
                  decoration: BoxDecoration(
                    gradient: RadialGradient(
                      center: const Alignment(0.92, 0.92),
                      radius: 1.1,
                      colors: [
                        Colors.black.withValues(alpha: 0.30),
                        Colors.black.withValues(alpha: 0.62),
                      ],
                    ),
                  ),
                ),
              ),
            ),
          ),
        Positioned(
          right: 18,
          bottom: MediaQuery.of(context).padding.bottom + 20,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.end,
            children: [
              ...fanItems,
              const SizedBox(height: 20),
              _buildGear(),
            ],
          ),
        ),
      ],
    );
  }

  Widget _buildFanItem(MainNavTab tab, int slotFromBottom) {
    final (outline, filled, label, accent) = _meta(tab);
    final selected = tab == widget.currentTab;
    final disabled = widget.disabledTabs.contains(tab);
    return _buildMenuItem(
      slotFromBottom: slotFromBottom,
      outline: outline,
      filled: filled,
      label: label,
      accent: accent,
      selected: selected,
      disabled: disabled,
      onTap: () => _select(tab),
    );
  }

  Widget _buildReportBugFanItem(int slotFromBottom) {
    return _buildMenuItem(
      slotFromBottom: slotFromBottom,
      outline: Icons.bug_report_outlined,
      filled: Icons.bug_report_rounded,
      label: 'Report Bug',
      accent: const Color(0xFF26C6DA),
      onTap: _reportBug,
    );
  }

  Widget _buildMenuItem({
    required int slotFromBottom,
    required IconData outline,
    required IconData filled,
    required String label,
    required Color accent,
    required VoidCallback onTap,
    bool selected = false,
    bool disabled = false,
  }) {
    // Items nearest the gear spring in first.
    final start = (slotFromBottom * 0.09).clamp(0.0, 0.55);
    final anim = CurvedAnimation(
      parent: _controller,
      curve: Interval(start, 1.0, curve: Curves.easeOutBack),
      reverseCurve: const Interval(0.0, 1.0, curve: Curves.easeInCubic),
    );

    final iconColor = disabled
        ? CelestialColors.textSecondary.withValues(alpha: 0.35)
        : accent;
    final textColor = disabled
        ? CelestialColors.textSecondary.withValues(alpha: 0.35)
        : selected
            ? accent
            : CelestialColors.textPrimary;

    return AnimatedBuilder(
      animation: anim,
      builder: (context, child) {
        final v = anim.value.clamp(0.0, 1.0);
        return Opacity(
          opacity: v,
          child: Transform.translate(
            offset: Offset((1 - v) * 18, 0),
            child: Transform.scale(
              scale: 0.92 + 0.08 * v,
              alignment: Alignment.centerRight,
              child: child,
            ),
          ),
        );
      },
      child: IgnorePointer(
        ignoring: !_open,
        child: Padding(
          padding: const EdgeInsets.only(bottom: 14),
          child: Semantics(
            button: true,
            selected: selected,
            label: label,
            child: Material(
              color: Colors.transparent,
              borderRadius: BorderRadius.circular(26),
              clipBehavior: Clip.antiAlias,
              child: InkWell(
                onTap: disabled ? null : onTap,
                child: Container(
                  height: 50,
                  padding: const EdgeInsets.fromLTRB(20, 0, 20, 0),
                  decoration: BoxDecoration(
                    borderRadius: BorderRadius.circular(26),
                    gradient: LinearGradient(
                      begin: Alignment.topLeft,
                      end: Alignment.bottomRight,
                      colors: selected
                          ? [
                              Color.lerp(CelestialColors.backgroundCard, accent,
                                  0.22)!,
                              Color.lerp(CelestialColors.backgroundCard, accent,
                                  0.10)!,
                            ]
                          : [
                              CelestialColors.backgroundCard
                                  .withValues(alpha: 0.96),
                              CelestialColors.backgroundCard
                                  .withValues(alpha: 0.86),
                            ],
                    ),
                    border: Border.all(
                      color: accent.withValues(
                        alpha: disabled
                            ? 0.15
                            : selected
                                ? 0.6
                                : 0.3,
                      ),
                      width: 1,
                    ),
                    boxShadow: [
                      if (selected)
                        BoxShadow(
                          color: accent.withValues(alpha: 0.40),
                          blurRadius: 20,
                          spreadRadius: -4,
                        ),
                      BoxShadow(
                        color: Colors.black.withValues(alpha: 0.35),
                        blurRadius: 12,
                        offset: const Offset(0, 5),
                      ),
                    ],
                  ),
                  child: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Icon(selected ? filled : outline,
                          size: 20, color: iconColor),
                      const SizedBox(width: 11),
                      Text(
                        label,
                        style: TextStyle(
                          color: textColor,
                          fontSize: 15,
                          fontWeight: FontWeight.w600,
                          letterSpacing: 0.2,
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget _buildGear() {
    const accent = CelestialColors.accentBlue;
    return Semantics(
      button: true,
      label: _open ? 'Close navigation' : 'Open navigation',
      child: Material(
        color: Colors.transparent,
        shape: const CircleBorder(),
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          onTap: _toggle,
          child: AnimatedBuilder(
            animation: _controller,
            builder: (context, child) {
              final t = _controller.value;
              return Container(
                width: 52,
                height: 52,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: CelestialColors.backgroundCard.withValues(alpha: 0.9),
                  boxShadow: [
                    BoxShadow(
                      color: accent.withValues(alpha: 0.20 + 0.28 * t),
                      blurRadius: 16 + 8 * t,
                      spreadRadius: -3,
                    ),
                    BoxShadow(
                      color: Colors.black.withValues(alpha: 0.30),
                      blurRadius: 8,
                      offset: const Offset(0, 3),
                    ),
                  ],
                  border: Border.all(
                    color: accent.withValues(alpha: 0.4 + 0.25 * t),
                    width: 1,
                  ),
                ),
                child: child,
              );
            },
            child: Center(
              child: AnimatedSwitcher(
                duration: const Duration(milliseconds: 250),
                transitionBuilder: (child, anim) => RotationTransition(
                  turns: Tween(begin: 0.55, end: 1.0).animate(anim),
                  child: FadeTransition(opacity: anim, child: child),
                ),
                child: Icon(
                  _open ? Icons.close_rounded : Icons.settings_rounded,
                  key: ValueKey(_open),
                  color: accent,
                  size: 25,
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// Small floating "+" orb pinned bottom-left above the nav bar. Opens the Add
/// Device flow (pair a hub, Matter device, etc.); the cool cyan glow ties it to
/// the "Hardware" language used across Settings.
class _FloatingDevicesButton extends StatelessWidget {
  const _FloatingDevicesButton({required this.onTap, this.badgeCount = 0});

  final VoidCallback onTap;

  /// Pending device-review count — shown as an amber badge (moved here from the
  /// old notifications bell, since "+" now opens the Add & Review screen).
  final int badgeCount;

  @override
  Widget build(BuildContext context) {
    const accent = Color(0xFF35D0E8);
    return Semantics(
      button: true,
      label:
          badgeCount > 0 ? 'Add & review, $badgeCount pending' : 'Add a device',
      child: Stack(
        clipBehavior: Clip.none,
        children: [
          Material(
            color: Colors.transparent,
            shape: const CircleBorder(),
            clipBehavior: Clip.antiAlias,
            child: InkWell(
              onTap: () {
                HapticFeedback.selectionClick();
                onTap();
              },
              child: Container(
                width: 52,
                height: 52,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: CelestialColors.backgroundCard.withValues(alpha: 0.9),
                  border: Border.all(
                    color: accent.withValues(alpha: 0.4),
                    width: 1,
                  ),
                  boxShadow: [
                    BoxShadow(
                      color: accent.withValues(alpha: 0.22),
                      blurRadius: 16,
                      spreadRadius: -3,
                    ),
                    BoxShadow(
                      color: Colors.black.withValues(alpha: 0.30),
                      blurRadius: 8,
                      offset: const Offset(0, 2),
                    ),
                  ],
                ),
                child: const Icon(
                  Icons.add_rounded,
                  color: accent,
                  size: 26,
                ),
              ),
            ),
          ),
          if (badgeCount > 0)
            Positioned(
              right: -2,
              top: -2,
              child: Container(
                padding: const EdgeInsets.symmetric(horizontal: 5, vertical: 1),
                constraints: const BoxConstraints(minWidth: 18),
                decoration: BoxDecoration(
                  color: const Color(0xFFFF9800),
                  borderRadius: BorderRadius.circular(9),
                  border: Border.all(
                    color: CelestialColors.backgroundDark,
                    width: 2,
                  ),
                ),
                child: Text(
                  badgeCount > 9 ? '9+' : '$badgeCount',
                  textAlign: TextAlign.center,
                  style: const TextStyle(
                    color: Color(0xFF1A1A1A),
                    fontSize: 11,
                    fontWeight: FontWeight.w800,
                    height: 1.2,
                  ),
                ),
              ),
            ),
        ],
      ),
    );
  }
}
