import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show
        RhythmConnection,
        RhythmConnectionState,
        RhythmConstantCurve,
        RhythmCurveConfig,
        RhythmMode,
        RhythmModeConfig,
        RhythmSuperGaussianCurve;
import 'models/config_model.dart';
import 'providers/room_provider.dart';
import 'providers/home_provider.dart';
import 'providers/room_page_provider.dart';
import 'providers/server_sync_provider.dart';
import 'screens/all_rooms_screen.dart';
import 'screens/server_disconnected_screen.dart';
import 'screens/settings/default_transition_editor_screen.dart';
import 'screens/settings/light_profile_screen.dart';
import 'screens/settings/settings_screen.dart';
import 'services/analytics_service.dart';
import 'services/app_state_refresh.dart';
import 'services/hue/hue_service_locator.dart';
import 'utils/room_visibility.dart';
import 'widgets/connect_hub_screen.dart';
import 'widgets/hardware_gate_screen.dart';
import 'widgets/hub_picker_screen.dart';
import 'widgets/main_bottom_nav.dart';
import 'widgets/solar_orbit.dart';

/// Main app shell.
///
/// Persistent 5-tab bottom navigation bar with an [IndexedStack] body.  Each
/// tab owns its own [Navigator] so deeper pushes (e.g. Settings → Lights &
/// Devices) stay inside the body slot and the navbar remains visible.
class AppShell extends StatefulWidget {
  const AppShell({super.key});

  @override
  State<AppShell> createState() => _AppShellState();
}

class _AppShellState extends State<AppShell> with WidgetsBindingObserver {
  static const _tabs = [
    MainNavTab.home,
    MainNavTab.dailyRhythm,
    MainNavTab.day,
    MainNavTab.sleep,
    MainNavTab.settings,
  ];

  int _tabIndex = 0;
  final Set<int> _builtTabIndexes = {0};
  late final List<GlobalKey<NavigatorState>> _navKeys =
      List.generate(_tabs.length, (_) => GlobalKey<NavigatorState>());

  CurveData? _curveData;
  final PageController _roomPageController = PageController();
  bool _hadServerHub = false;
  bool _serverRemovalCleanupPending = false;

  /// Sticky flag governing the [ServerDisconnectedScreen]. Set true when we
  /// can't reach the server *and* have never completed a hello with it (so
  /// we don't flash the disconnect screen during the transient reconnect
  /// that fires after every save action). Cleared on [RhythmConnectionState.connected].
  bool _serverLostConnection = false;

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
    WidgetsBinding.instance.removeObserver(this);
    _roomPageController.dispose();
    super.dispose();
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (state == AppLifecycleState.resumed) {
      _pingServer();
    }
  }

  void _pingServer() {
    try {
      final conn = context.read<RhythmConnection>();
      conn.pingOrReconnect();
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
      _navKeys[newIndex].currentState?.popUntil((route) => route.isFirst);
      return;
    }

    setState(() {
      _tabIndex = newIndex;
      _builtTabIndexes.add(newIndex);
    });
    AnalyticsService().logScreenView(_screenNameFor(tab));
  }

  String _screenNameFor(MainNavTab tab) => switch (tab) {
        MainNavTab.home => 'home',
        MainNavTab.dailyRhythm => 'daily_rhythm',
        MainNavTab.day => 'day_profile',
        MainNavTab.sleep => 'sleep_profile',
        MainNavTab.settings => 'settings',
      };

  /// Intercepts the system back button:
  ///   1. pop the active tab's nested stack if possible,
  ///   2. else fall back to the Home tab,
  ///   3. else allow the OS to exit the app.
  Future<void> _handlePopInvoked(bool didPop) async {
    if (didPop) return;
    final nav = _navKeys[_tabIndex].currentState;
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
      // Clear any nested-tab navigation pushed on top of the home tab so the
      // user lands back on the room grid.
      _navKeys[0].currentState?.popUntil((route) => route.isFirst);
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

  /// Flip the global mode from the All Rooms toggle.
  Future<void> _setActiveMode(RhythmMode mode) async {
    HapticFeedback.mediumImpact();
    final roomProvider = context.read<RoomProvider>();
    if (roomProvider.anyRoomTransitioning) return;
    final serverSync = context.read<ServerSyncProvider>();
    final currentMode = serverSync.activeMode;
    if (currentMode == mode) return;

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
    AnalyticsService().logGlobalModeChanged(
      mode.name,
      source: 'all_rooms_toggle',
    );
  }

  Future<void> _confirmReapplyActiveMode(RhythmMode mode) async {
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
    final roomProvider = context.read<RoomProvider>();
    if (roomProvider.anyRoomTransitioning) return;

    final serverSync = context.read<ServerSyncProvider>();
    final modeLabel = _modeLabel(mode);

    if (!serverSync.canDispatchActions) {
      _showModeActionError('Could not reapply $modeLabel settings.');
      return;
    }

    HapticFeedback.mediumImpact();

    final ranTransition = await serverSync
        .dispatchRunTransition(_defaultTransitionIdForMode(mode));
    if (ranTransition) {
      if (mounted) HapticFeedback.heavyImpact();
      return;
    }

    _showModeActionError('Could not reapply $modeLabel settings.');
  }

  @override
  Widget build(BuildContext context) {
    final hasServerHub = context.select<HomeProvider, bool>(
      (homeProvider) => homeProvider.getFirstHubOfType(HubType.server) != null,
    );
    _handleServerHubLifecycle(hasServerHub: hasServerHub);

    // `hasBeenSynced` is the sticky flag (true once a hello has completed,
    // false only on full hub unpair). Reading the non-sticky `synced` here
    // would dip false during transient SSE/poll reconnects — including the
    // reconnect that fires right after any HTTP action against the server
    // (e.g. toggling the Time/Button trigger switches or starting a listen
    // for a button press), evicting the user from the Transitions tab back
    // to Home mid-interaction.
    final serverSynced = context.select<ServerSyncProvider, bool>(
      (s) => s.hasBeenSynced,
    );
    final visibleTabs = _computeVisibleTabs(serverSynced: serverSynced);

    // If the current tab depends on a synced server (e.g. Daily Rhythm) and
    // the server hub was actually unpaired, bounce the user back to Home.
    if (!visibleTabs.contains(_currentTab)) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (!mounted) return;
        if (!_computeVisibleTabs(
                serverSynced: context.read<ServerSyncProvider>().hasBeenSynced)
            .contains(_currentTab)) {
          setState(() {
            _tabIndex = 0;
            _builtTabIndexes.add(0);
          });
        }
      });
    }

    return PopScope(
      canPop: false,
      onPopInvokedWithResult: (didPop, _) => _handlePopInvoked(didPop),
      child: Scaffold(
        backgroundColor: CelestialColors.backgroundDark,
        body: Stack(
          fit: StackFit.expand,
          children: List.generate(
            _tabs.length,
            _buildTabSlot,
          ),
        ),
        bottomNavigationBar: _buildBottomNav(visibleTabs: visibleTabs),
      ),
    );
  }

  /// Bottom-nav tab visibility — Daily Rhythm is hidden until the Rhythm
  /// server has completed at least one hello (otherwise the orbital editor
  /// renders without real profile data and looks wrong). Uses a sticky
  /// `hasBeenSynced` signal so the tab does not flicker out during
  /// transient reconnects (e.g. pull-to-refresh).
  List<MainNavTab> _computeVisibleTabs({required bool serverSynced}) {
    return [
      MainNavTab.home,
      if (serverSynced) MainNavTab.dailyRhythm,
      MainNavTab.day,
      MainNavTab.sleep,
      MainNavTab.settings,
    ];
  }

  Widget _buildTabSlot(int index) {
    final isActive = index == _tabIndex;
    return Offstage(
      offstage: !isActive,
      child: TickerMode(
        enabled: isActive,
        child: _builtTabIndexes.contains(index)
            ? _TabNavigator(
                navigatorKey: _navKeys[index],
                builder: (_) => _buildTabRoot(_tabs[index]),
              )
            : const SizedBox.shrink(),
      ),
    );
  }

  Widget _buildTabRoot(MainNavTab tab) {
    return switch (tab) {
      MainNavTab.home => _buildHomeTab(),
      MainNavTab.dailyRhythm => _buildDailyRhythmTab(),
      MainNavTab.day => const LightProfileScreen(initialProfile: 'rhythm'),
      MainNavTab.sleep => const LightProfileScreen(initialProfile: 'sleep'),
      MainNavTab.settings => const SettingsScreen(),
    };
  }

  Widget _buildBottomNav({required List<MainNavTab> visibleTabs}) {
    return MainBottomNav(
      currentBodyTab: _currentTab,
      onTabSelected: _handleTabSelected,
      tabs: visibleTabs,
    );
  }

  /// Daily Rhythm tab — wraps the editor in a [Consumer] so [profileColors]
  /// stays in sync with the active server profiles even if they change while
  /// the tab is alive.
  Widget _buildDailyRhythmTab() {
    return Consumer<ServerSyncProvider>(
      builder: (context, serverSync, _) {
        return DefaultTransitionEditorScreen(
          profileColors: _resolveProfileColors(
            serverSync.modeConfigs,
            serverSync.profiles,
          ),
        );
      },
    );
  }

  /// Resolves the body for the Home tab through the existing
  /// connection/rooms state machine.
  Widget _buildHomeTab() {
    return Consumer3<RoomProvider, HomeProvider, ServerSyncProvider>(
      builder: (context, roomProvider, homeProvider, serverSync, child) {
        final serverHub = homeProvider.getFirstHubOfType(HubType.server);
        final state = serverSync.connectionState;

        // Govern the disconnect screen:
        //   • `connected`              → clear the flag, show the room grid.
        //   • `disconnected`           → flag it, hub is truly gone.
        //   • `reconnecting` (initial, never synced)  → flag it (hub looks
        //     unreachable from cold start).
        //   • `reconnecting` (after a clean sync)     → do NOT flag — this
        //     is the transient blip that fires after every HTTP action and
        //     would otherwise flash the disconnect screen on every save.
        //   • `connecting` (first-attempt phase)      → leave alone so the
        //     "Setting up…" connecting state can render.
        if (state == RhythmConnectionState.connected) {
          _serverLostConnection = false;
        } else if (state == RhythmConnectionState.disconnected ||
            (state == RhythmConnectionState.reconnecting &&
                !serverSync.hasBeenSynced)) {
          _serverLostConnection = true;
        }

        if (serverHub != null) {
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
          pendingMode:
              roomProvider.anyRoomTransitioning ? serverSync.activeMode : null,
          onModeSelected: _setActiveMode,
          onActiveModeDoubleTap: _confirmReapplyActiveMode,
        );
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

/// Maps each [RhythmMode] to the dominant color of its active profile, used to
/// tint controls in [DefaultTransitionEditorScreen].
Map<RhythmMode, Color> _resolveProfileColors(
  List<RhythmModeConfig> modeConfigs,
  List<RhythmCurveConfig> profiles,
) {
  final colors = <RhythmMode, Color>{};
  for (final mc in modeConfigs) {
    final profile = profiles.cast<RhythmCurveConfig?>().firstWhere(
          (p) => p!.id == mc.activeProfileId,
          orElse: () => null,
        );
    if (profile == null) continue;
    colors[mc.mode] = _colorFromProfile(profile);
  }
  return colors;
}

Color _colorFromProfile(RhythmCurveConfig profile) {
  final curve = profile.curve;
  if (curve is RhythmConstantCurve && curve.directColor != null) {
    final rgb = curve.directColor!.rgb;
    return Color.fromARGB(255, rgb.r, rgb.g, rgb.b);
  }
  if (curve is RhythmSuperGaussianCurve && curve.directColor != null) {
    final rgb = curve.directColor!.rgb;
    return Color.fromARGB(255, rgb.r, rgb.g, rgb.b);
  }
  final midCct = (profile.minColorTemp + profile.maxColorTemp) ~/ 2;
  return ColorUtils.cctToColor(midCct);
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
