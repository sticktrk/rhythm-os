import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show
        RhythmConnectionState,
        RhythmLightRuntime,
        RhythmLightRuntimePresentation,
        RhythmMode;
import 'models/config_model.dart';
import 'backend/auth/auth_user.dart';
import 'providers/room_provider.dart';
import 'providers/home_provider.dart';
import 'providers/hub_connection_provider.dart';
import 'providers/room_page_provider.dart';
import 'providers/server_sync_provider.dart';
import 'screens/all_rooms_screen.dart';
import 'screens/server_disconnected_screen.dart';
import 'screens/settings/automations_screen.dart';
import 'screens/settings/dialogs/sign_in_modal.dart';
import 'screens/settings/light_screen.dart';
import 'screens/settings/sections/lights_devices_section.dart';
import 'screens/settings/settings_screen.dart';
import 'screens/sun_position_screen.dart';
import 'features/circadian_expert/circadian_expert_screen.dart';
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
import 'widgets/hardware_gate_screen.dart';
import 'widgets/hub_picker_screen.dart';
import 'widgets/main_bottom_nav.dart';
import 'widgets/solar_orbit.dart';
import 'widgets/virtual_experience_banner.dart';

const _removed-projectAccent = Color(0xFFFEAC60);

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

  static const _rhythmAdaptiveTabs = [
    MainNavTab.home,
    MainNavTab.light,
    MainNavTab.automations,
    MainNavTab.devices,
    MainNavTab.settings,
  ];

  static const _runtimeShellTabs = [
    MainNavTab.runtimeHome,
    MainNavTab.runtimeRhythm,
    MainNavTab.runtimeMoments,
    MainNavTab.runtimeControls,
    MainNavTab.runtimeSettings,
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
  bool _runtimeShellActive = false;
  bool _lightRuntimeSwitching = false;
  bool? _lightRuntimeSwitchTarget;
  RhythmMode? _pendingModeAction;
  Timer? _pendingModeActionClearTimer;
  RoomProvider? _pendingModeActionRoomProvider;
  VoidCallback? _pendingModeActionRoomListener;
  bool _accountActionInProgress = false;

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

  List<MainNavTab> get _activeTabs =>
      _runtimeShellActive ? _runtimeShellTabs : _rhythmAdaptiveTabs;

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

      final homeName = homeProvider.currentHome?.name ?? 'Home';
      unawaited(
        context
            .read<ServerSyncProvider>()
            .refreshForHomeEntry(homeName: homeName),
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
        MainNavTab.runtimeHome => 'runtime_home',
        MainNavTab.runtimeRhythm => 'runtime_rhythm',
        MainNavTab.runtimeMoments => 'runtime_moments',
        MainNavTab.runtimeControls => 'runtime_controls',
        MainNavTab.runtimeSettings => 'runtime_settings',
      };

  void _showremoved-projectRuntimeHome() {
    setState(() {
      _runtimeShellActive = true;
      _tabIndex = 0;
      _builtTabs.add(MainNavTab.runtimeHome);
    });
    AnalyticsService().logScreenView(_screenNameFor(MainNavTab.runtimeHome));
  }

  void _showRhythmAdaptiveRuntimeHome() {
    setState(() {
      _runtimeShellActive = false;
      _tabIndex = 0;
      _builtTabs.add(MainNavTab.home);
    });
    AnalyticsService().logScreenView(_screenNameFor(MainNavTab.home));
  }

  void _syncLightRuntimeUiFromServer(
    RhythmLightRuntime lightRuntime,
    bool serverSynced,
  ) {
    if (_lightRuntimeSwitching) return;
    if (!serverSynced && !HueServiceLocator.isDemoMode) return;
    final shouldUseRuntimeShell = lightRuntime.usesRuntimeShell;
    if (_runtimeShellActive == shouldUseRuntimeShell) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || _runtimeShellActive == shouldUseRuntimeShell) return;
      if (shouldUseRuntimeShell) {
        _showremoved-projectRuntimeHome();
      } else {
        _showRhythmAdaptiveRuntimeHome();
      }
    });
  }

  Future<bool> _confirmLightRuntimeSwitch({
    required bool toremoved-projectRuntime,
  }) async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: Text(
          toremoved-projectRuntime
              ? 'Switch to Circadian Expert Mode?'
              : 'Switch to Basic Mode?',
        ),
        content: Text(
          toremoved-projectRuntime
              ? 'Your Basic settings stay saved. Lights will transition to the expert sigmoid profile.'
              : 'Your Expert settings stay saved. Lights will transition back to the Basic super-Gaussian profile.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(false),
            child: const Text('Cancel'),
          ),
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(true),
            child:
                Text(toremoved-projectRuntime ? 'Switch to Expert' : 'Switch to Basic'),
          ),
        ],
      ),
    );
    return confirmed == true;
  }

  void _showLightRuntimeSwitchError(bool toremoved-projectRuntime) {
    if (!mounted) return;
    ScaffoldMessenger.maybeOf(context)?.showSnackBar(
      SnackBar(
        content: Text(
          toremoved-projectRuntime
              ? 'Could not switch to Expert Mode.'
              : 'Could not switch to Basic Mode.',
        ),
      ),
    );
  }

  Future<void> _selectremoved-projectRuntime() async {
    if (_runtimeShellActive || _lightRuntimeSwitching) return;
    final confirmed = await _confirmLightRuntimeSwitch(toremoved-projectRuntime: true);
    if (!confirmed || !mounted) return;

    setState(() {
      _lightRuntimeSwitching = true;
      _lightRuntimeSwitchTarget = true;
    });
    HapticFeedback.heavyImpact();
    try {
      final success = await context
          .read<ServerSyncProvider>()
          .dispatchSetLightRuntime(RhythmLightRuntime.removed-projectCircadian);
      if (!success) {
        _showLightRuntimeSwitchError(true);
        return;
      }
      if (mounted) _showremoved-projectRuntimeHome();
    } finally {
      if (mounted) {
        setState(() {
          _lightRuntimeSwitching = false;
          _lightRuntimeSwitchTarget = null;
        });
      }
    }
  }

  Future<void> _selectRhythmAdaptiveRuntime() async {
    if (!_runtimeShellActive || _lightRuntimeSwitching) return;
    final confirmed = await _confirmLightRuntimeSwitch(toremoved-projectRuntime: false);
    if (!confirmed || !mounted) return;

    setState(() {
      _lightRuntimeSwitching = true;
      _lightRuntimeSwitchTarget = false;
    });
    HapticFeedback.mediumImpact();
    try {
      final success = await context
          .read<ServerSyncProvider>()
          .dispatchSetLightRuntime(RhythmLightRuntime.rhythmAdaptive);
      if (!success) {
        _showLightRuntimeSwitchError(false);
        return;
      }
      if (mounted) _showRhythmAdaptiveRuntimeHome();
    } finally {
      if (mounted) {
        setState(() {
          _lightRuntimeSwitching = false;
          _lightRuntimeSwitchTarget = null;
        });
      }
    }
  }

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
    if (_runtimeShellActive) {
      await _selectRhythmAdaptiveRuntime();
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
        _runtimeShellActive = false;
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
    final lightRuntime = context.select<ServerSyncProvider, RhythmLightRuntime>(
      (s) => s.lightRuntime,
    );
    _syncLightRuntimeUiFromServer(lightRuntime, serverSynced);
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
            transitionBuilder: _lightRuntimeFlipTransition,
            child: Scaffold(
              key: ValueKey<bool>(_runtimeShellActive),
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
                              _activeTabs.length,
                              _buildTabSlot,
                            ),
                          ),
                        ),
                      ),
                    ],
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
                  if (!hideChrome)
                    Positioned(
                      right: 14,
                      bottom: 14,
                      child: _FloatingSunButton(
                        onTap: () => SunPositionScreen.show(context),
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
                  if (_lightRuntimeSwitching)
                    Positioned.fill(
                      child: _LightRuntimeSwitchOverlay(
                        toremoved-projectRuntime:
                            _lightRuntimeSwitchTarget ?? !_runtimeShellActive,
                      ),
                    ),
                ],
              ),
              bottomNavigationBar: hideChrome
                  ? null
                  : _buildBottomNav(
                      visibleTabs: visibleTabs,
                      disabledTabs: disabledTabs,
                    ),
            ),
          );
        },
      ),
    );
  }

  Widget _lightRuntimeFlipTransition(
    Widget child,
    Animation<double> animation,
  ) {
    final incoming = child.key == ValueKey<bool>(_runtimeShellActive);
    return AnimatedBuilder(
      animation: animation,
      child: child,
      builder: (context, child) {
        final turn = 1 - animation.value;
        final angle = turn * math.pi * 0.5 * (incoming ? 1 : -1);
        final scale = 0.94 + animation.value * 0.06;
        return Transform(
          alignment: Alignment.center,
          transform: Matrix4.identity()
            ..setEntry(3, 2, 0.0015)
            ..rotateY(angle),
          child: Transform.scale(
            scale: scale,
            child: child,
          ),
        );
      },
    );
  }

  /// Bottom-nav tab visibility — all five tabs are always rendered so the
  /// navbar shape stays stable. Tabs whose dependencies aren't met get
  /// disabled via [_computeDisabledTabs] instead of being hidden.
  List<MainNavTab> _computeVisibleTabs({required bool serverSynced}) {
    return _activeTabs;
  }

  /// Tabs to grey-out and reject taps on. Automations is disabled until the
  /// Rhythm server has completed at least one hello — the orbital editor
  /// needs real profile data to render meaningfully. Uses the sticky
  /// `hasBeenSynced` signal so the disabled state doesn't flicker on
  /// transient reconnects.
  Set<MainNavTab> _computeDisabledTabs({required bool serverSynced}) {
    if (_runtimeShellActive) return const {};
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
      MainNavTab.settings => SettingsScreen(
          onSelectremoved-projectCircadianRuntime: () {
            unawaited(_selectremoved-projectRuntime());
          },
        ),
      MainNavTab.runtimeHome => const CircadianExpertScreen(
          showBackButton: false,
          showToolStrip: false,
        ),
      MainNavTab.runtimeRhythm => const CircadianExpertRhythmScreen(),
      MainNavTab.runtimeMoments => const CircadianExpertMomentsTabScreen(),
      MainNavTab.runtimeControls => const CircadianExpertControlsTabScreen(),
      MainNavTab.runtimeSettings => SettingsScreen(
          runtimeShellActive: true,
          onSelectRhythmAdaptiveRuntime: () {
            unawaited(_selectRhythmAdaptiveRuntime());
          },
        ),
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
      runtimeShellActive: _runtimeShellActive,
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
          if (_serverLostConnection) {
            return ServerDisconnectedScreen(
              serverHub: serverHub,
              onChooseHome: () => ConnectHubScreen.show(
                context,
                mode: ConnectHubMode.rhythmServer,
              ),
            );
          }

          if (!serverSync.hasBeenSynced) {
            return _buildServerConnectingState();
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

    return ServerDisconnectedScreen(
      serverHub: serverHub,
      title: error == null ? displayHome : 'Server Unreachable',
      autoRetry: error != null,
      onRetry: error == null
          ? null
          : () async {
              await serverSync.refreshForHomeEntry(
                homeName: displayHome,
                allowWifiFastPath: false,
              );
            },
      // Escape hatch so a hung handshake never traps the user: drop the gate
      // and open the Home chooser.
      onChooseHome: () {
        serverSync.cancelHomeEntryRefresh();
        ConnectHubScreen.show(context, mode: ConnectHubMode.rhythmServer);
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

class _LightRuntimeSwitchOverlay extends StatelessWidget {
  final bool toremoved-projectRuntime;

  const _LightRuntimeSwitchOverlay({required this.toremoved-projectRuntime});

  @override
  Widget build(BuildContext context) {
    final title =
        toremoved-projectRuntime ? 'Applying Expert profile' : 'Applying Basic profile';
    return AbsorbPointer(
      child: DecoratedBox(
        decoration: BoxDecoration(
          color: CelestialColors.backgroundDark.withValues(alpha: 0.90),
        ),
        child: SafeArea(
          child: Center(
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 32),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  _PulsingIcon(
                    icon: toremoved-projectRuntime
                        ? Icons.auto_awesome_rounded
                        : Icons.lightbulb_rounded,
                    color: toremoved-projectRuntime
                        ? _removed-projectAccent
                        : CelestialColors.accentBlue,
                  ),
                  const SizedBox(height: 24),
                  Text(
                    title,
                    textAlign: TextAlign.center,
                    style: const TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 22,
                      fontWeight: FontWeight.w700,
                      letterSpacing: 0,
                    ),
                  ),
                  const SizedBox(height: 12),
                  Text(
                    'Syncing lights',
                    textAlign: TextAlign.center,
                    style: TextStyle(
                      color:
                          CelestialColors.textSecondary.withValues(alpha: 0.78),
                      fontSize: 15,
                      height: 1.45,
                      letterSpacing: 0,
                    ),
                  ),
                  const SizedBox(height: 24),
                  SizedBox(
                    width: 28,
                    height: 28,
                    child: CircularProgressIndicator(
                      strokeWidth: 2.4,
                      valueColor: AlwaysStoppedAnimation<Color>(
                        (toremoved-projectRuntime
                                ? _removed-projectAccent
                                : CelestialColors.accentBlue)
                            .withValues(alpha: 0.86),
                      ),
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
