import 'dart:io' show Platform;
import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'models/config_model.dart';
import 'providers/room_provider.dart';
import 'providers/home_provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show RhythmConnection, RhythmConnectionState, RhythmMode;
import 'screens/designer_screen.dart';
import 'screens/mobile_designer_screen.dart';
import 'providers/room_page_provider.dart';
import 'screens/all_rooms_screen.dart';
import 'screens/sun_position_screen.dart';
import 'screens/settings/settings_screen.dart';
import 'widgets/solar_orbit.dart';
import 'widgets/bottom_nav_overlay.dart';
import 'widgets/connect_hub_screen.dart';
import 'widgets/hub_picker_screen.dart';
import 'providers/server_sync_provider.dart';
import 'screens/server_disconnected_screen.dart';
import 'services/analytics_service.dart';
import 'services/app_state_refresh.dart';
import 'services/hue/hue_service_locator.dart';
import 'utils/room_visibility.dart';

/// Main app shell with constellation grid layout.
///
/// Features:
/// - Scrollable grid of room orbs (constellation view)
/// - Settings gear (bottom-left) opening a settings modal
/// - Falls back to single orbit view when no rooms are synced
class AppShell extends StatefulWidget {
  const AppShell({super.key});

  @override
  State<AppShell> createState() => _AppShellState();
}

class _AppShellState extends State<AppShell> with WidgetsBindingObserver {
  int _currentPage = 0;
  CurveData? _curveData;
  bool _isFixing = false;
  final PageController _roomPageController = PageController();
  int _currentRoomPage = 0;
  bool _hadServerHub = false;
  bool _serverRemovalCleanupPending = false;

  /// Sticky flag: true once the server enters [RhythmConnectionState.reconnecting],
  /// cleared when [RhythmConnectionState.connected] is reached.  Prevents flashing
  /// the room grid during the brief `connecting` phase of a reconnect cycle.
  bool _serverLostConnection = false;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    _loadData();
    // Track initial screen view
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

  /// Check if we're on a large screen (not macOS/iOS/web)
  bool _isLargeScreen(BuildContext context) {
    // Web uses mobile layout (rooms grid + settings)
    if (kIsWeb) return false;
    // macOS and iOS use mobile layout (iPad behaves like iPhone)
    if (Platform.isMacOS || Platform.isIOS) return false;
    final size = MediaQuery.of(context).size;
    final shortestSide = size.shortestSide;
    // Tablet threshold is typically 600dp
    return shortestSide >= 600;
  }

  void _openSettings() {
    SettingsScreen.show(context);
  }

  void _openSunPosition() {
    SunPositionScreen.show(context);
  }

  void _handleServerHubLifecycle({required bool hasServerHub}) {
    final serverRemoved = _hadServerHub && !hasServerHub;
    _hadServerHub = hasServerHub;

    if (!serverRemoved || _serverRemovalCleanupPending) return;
    _serverRemovalCleanupPending = true;

    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      final navigator = Navigator.maybeOf(context, rootNavigator: true);
      if (navigator != null && navigator.canPop()) {
        navigator.popUntil((route) => route.isFirst);
      }
      _serverRemovalCleanupPending = false;
    });
  }

  /// Reset all on-lights to current adaptive values and enable rhythm tracking.
  ///
  /// Dispatches node resets for every currently-on light-addressable node.
  Future<void> _fixMyLights() async {
    HapticFeedback.mediumImpact();
    setState(() => _isFixing = true);

    try {
      final serverSync = context.read<ServerSyncProvider>();
      await serverSync.dispatchFixMyLights();
      AnalyticsService().logFixMyLights(source: 'app_shell');
      if (mounted) HapticFeedback.heavyImpact();
    } finally {
      if (mounted) setState(() => _isFixing = false);
    }
  }

  /// Flip the global mode from the All Rooms toggle.
  ///
  /// For the day/sleep toggle we run the default saved transitions so the
  /// backend applies the canonical transition object rather than a direct mode
  /// switch.
  Future<void> _setActiveMode(RhythmMode mode) async {
    HapticFeedback.mediumImpact();
    final roomProvider = context.read<RoomProvider>();
    if (roomProvider.anyRoomTransitioning) return;
    final serverSync = context.read<ServerSyncProvider>();
    final currentMode = serverSync.activeMode;
    if (currentMode == mode) return;

    final transitionId = currentMode == null
        ? null
        : switch (mode) {
            RhythmMode.sleep => 'day_to_sleep',
            RhythmMode.day => 'sleep_to_day',
          };
    if (transitionId == null) {
      await serverSync.dispatchSetActiveMode(mode);
      await _loadData();
      AnalyticsService().logGlobalModeChanged(
        mode.name,
        source: 'all_rooms_toggle',
      );
      return;
    }

    final success = await serverSync.dispatchRunTransition(transitionId);
    if (!success) return;
    await _loadData();
    AnalyticsService().logGlobalModeChanged(
      mode.name,
      source: 'all_rooms_toggle',
    );
  }

  @override
  Widget build(BuildContext context) {
    final hasServerHub = context.select<HomeProvider, bool>(
      (homeProvider) => homeProvider.getFirstHubOfType(HubType.server) != null,
    );
    _handleServerHubLifecycle(hasServerHub: hasServerHub);

    final showSliders = _isLargeScreen(context);

    // For large screens, show the original bottom nav with sliders
    if (showSliders) {
      return _buildLargeScreenLayout();
    }

    // Mobile: All rooms constellation grid
    return Consumer3<RoomProvider, HomeProvider, ServerSyncProvider>(
      builder: (context, roomProvider, homeProvider, serverSync, child) {
        final serverHub = homeProvider.getFirstHubOfType(HubType.server);
        final state = serverSync.connectionState;

        // Track server connection loss across rebuild cycles.
        if (state == RhythmConnectionState.reconnecting) {
          _serverLostConnection = true;
        } else if (state == RhythmConnectionState.connected) {
          _serverLostConnection = false;
        }

        if (serverHub != null) {
          // ── Server unreachable ──────────────────────────────────
          // Show the disconnected screen once a reconnect cycle has
          // started, and keep showing it through subsequent connecting
          // attempts until the server is fully connected again.
          if (_serverLostConnection) {
            return ServerDisconnectedScreen(
              serverHub: serverHub,
              onSettingsTap: _openSettings,
              onSunPositionTap: _openSunPosition,
            );
          }

          // ── Server connected ────────────────────────────────────
          if (state == RhythmConnectionState.connected) {
            if (roomProvider.hasRooms) {
              return _buildRoomGrid(roomProvider);
            }

            // With a connected server but no synced rooms yet, keep the user on
            // the hub picker so they can add Matter devices immediately and see
            // which upstream hubs are already connected.
            return HubPickerScreen(
              onSettingsTap: _openSettings,
              onSunPositionTap: _openSunPosition,
            );
          }

          // ── First connection attempt (connecting / initial disconnected) ─
          if (!roomProvider.hasRooms) {
            return _buildServerConnectingState();
          }

          // Has cached rooms and server is doing its first connect —
          // show the room grid while the connection establishes.
          return _buildRoomGrid(roomProvider);
        }

        // ── No server hub paired ───────────────────────────────────
        if (!roomProvider.hasRooms) {
          if (HueServiceLocator.isDemoMode) {
            return _buildServerConnectingState();
          }
          return _buildNoRoomsLayout(roomProvider, ConnectHubMode.rhythmServer);
        }

        return _buildRoomGrid(roomProvider);
      },
    );
  }

  /// The main room constellation grid with bottom nav overlay.
  Widget _buildRoomGrid(RoomProvider roomProvider) {
    final serverSync = context.watch<ServerSyncProvider>();
    final roomPageProvider = context.watch<RoomPageProvider>();
    final enabledRooms = roomProvider.enabledRooms;
    final visibleRooms =
        enabledRooms.where(showsInAllRooms).toList(growable: false);
    final pageCount = roomPageProvider.pageCount;
    // Clamp current page if page count decreased
    if (_currentRoomPage >= pageCount) {
      _currentRoomPage = (pageCount - 1).clamp(0, pageCount - 1);
    }

    // Reconcile page assignments after frame to avoid notifying during build
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      context.read<RoomPageProvider>().reconcileRooms(
            visibleRooms,
          );
    });

    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: Stack(
        children: [
          // Main content - all rooms grid
          Consumer<ConfigModel>(
            builder: (context, configModel, _) {
              return AllRoomsScreen(
                rooms: visibleRooms,
                globalConfig: configModel.config,
                curveData: _curveData,
                pageController: _roomPageController,
                activeMode: serverSync.activeMode,
                pendingMode: roomProvider.anyRoomTransitioning
                    ? serverSync.activeMode
                    : null,
                onModeSelected: _setActiveMode,
                onPageChanged: (page) {
                  setState(() => _currentRoomPage = page);
                },
              );
            },
          ),
          // Bottom gradient fade + overlay
          Positioned(
            bottom: 0,
            left: 0,
            right: 0,
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                // Gradient fade so cards dissolve into the bar
                Container(
                  height: 40,
                  decoration: const BoxDecoration(
                    gradient: LinearGradient(
                      begin: Alignment.topCenter,
                      end: Alignment.bottomCenter,
                      colors: [
                        Color(0x00000000),
                        Color(0x800D1117),
                      ],
                    ),
                  ),
                ),
                // Solid bar behind controls
                Container(
                  color: const Color(0x800D1117),
                  child: SafeArea(
                    top: false,
                    child: BottomNavOverlay(
                      currentPage: _currentRoomPage,
                      totalPages: pageCount,
                      editMode: roomPageProvider.editMode,
                      pageController: _roomPageController,
                      onSettingsTap: _openSettings,
                      onSunPositionTap: _openSunPosition,
                      onFixMyLights: _fixMyLights,
                      isFixing: _isFixing,
                    ),
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  /// Server hub is connected/connecting but has no rooms yet (Hue discovery in progress).
  Widget _buildServerConnectingState() {
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: Stack(
        children: [
          SafeArea(
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
                        color: CelestialColors.textSecondary
                            .withValues(alpha: 0.7),
                        fontSize: 15,
                        height: 1.5,
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
          // Bottom overlay
          Positioned(
            bottom: 0,
            left: 0,
            right: 0,
            child: SafeArea(
              child: BottomNavOverlay(
                currentPage: 0,
                totalPages: 1,
                onSettingsTap: _openSettings,
                onSunPositionTap: _openSunPosition,
              ),
            ),
          ),
        ],
      ),
    );
  }

  /// Build layout when no rooms are synced.
  /// Full-screen onboarding centered on connecting a hub.
  Widget _buildNoRoomsLayout(RoomProvider roomProvider, ConnectHubMode mode) {
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: Stack(
        children: [
          // Full-screen connect hub experience
          SafeArea(
            bottom: false,
            child: ConnectHubScreen(mode: mode),
          ),
          // Bottom overlay (gear + sun button, no dots)
          Positioned(
            bottom: 0,
            left: 0,
            right: 0,
            child: SafeArea(
              child: BottomNavOverlay(
                currentPage: 0,
                totalPages: 1,
                onSettingsTap: _openSettings,
                onSunPositionTap: _openSunPosition,
              ),
            ),
          ),
        ],
      ),
    );
  }

  /// Build large screen layout with bottom navigation bar.
  /// Keeps the original design for tablets/web.
  Widget _buildLargeScreenLayout() {
    final screens = <Widget>[
      const MobileDesignerScreen(),
      const DesignerScreen(),
    ];

    final navItems = <BottomNavigationBarItem>[
      const BottomNavigationBarItem(
        icon: Icon(Icons.wb_sunny),
        label: 'Orbit',
      ),
      const BottomNavigationBarItem(
        icon: Icon(Icons.tune),
        label: 'Tuner',
      ),
    ];

    // Clamp index if screen size changed
    final safeIndex = _currentPage.clamp(0, screens.length - 1);

    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: IndexedStack(
        index: safeIndex,
        children: screens,
      ),
      bottomNavigationBar: BottomNavigationBar(
        currentIndex: safeIndex,
        onTap: (index) {
          // Track screen view change
          AnalyticsService().logScreenView(index == 0 ? 'orbit' : 'tuner');
          setState(() {
            _currentPage = index;
          });
        },
        type: BottomNavigationBarType.fixed,
        backgroundColor: CelestialColors.backgroundCard,
        selectedItemColor: CelestialColors.accentBlue,
        unselectedItemColor: CelestialColors.textSecondary,
        items: navItems,
      ),
    );
  }
}

/// Pulsing icon for the server-disconnected state.
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
