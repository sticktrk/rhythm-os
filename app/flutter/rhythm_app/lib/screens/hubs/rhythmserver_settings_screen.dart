import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show
        RhythmConfigApi,
        RhythmConnection,
        RhythmConnectionState,
        RhythmDevice,
        RhythmDeviceType,
        RhythmDiagnosticsApi,
        RhythmHubInfo,
        RhythmHubStartupRetry,
        RhythmHubStartupRetryStatus,
        RhythmRoom,
        RoomModeState;
import '../../widgets/solar_orbit.dart';
import '../../providers/server_sync_provider.dart';
import '../../providers/home_provider.dart';
import '../../providers/room_provider.dart';
import '../../services/analytics_service.dart';
import '../../services/ota_service.dart';
import '../../services/remote_access_service.dart';
import '../../services/server_endpoint_resolver.dart';
import '../../widgets/beta_badge.dart';
import '../../widgets/device_detail_sheet.dart';
import '../../widgets/info_tooltip.dart';
import '../../widgets/report_bug_flow.dart';
import '../settings/sections/lights_devices_section.dart';
import 'matter_pairing_flow.dart';
import 'ota_update_overlay.dart';

/// Settings screen for a connected server hub (bridge, standalone, HA addon).
///
/// Adapts visible sections based on the server's platform context.
class RhythmServerSettingsScreen extends StatefulWidget {
  final Hub hub;
  final String? headerTitleOverride;
  final bool useBackButton;
  final bool homeManaged;

  const RhythmServerSettingsScreen({
    super.key,
    required this.hub,
    this.headerTitleOverride,
    this.useBackButton = false,
    this.homeManaged = true,
  });

  /// Show as a full-screen modal with slide-up transition.
  static Future<void> show(
    BuildContext context, {
    required Hub hub,
    String? headerTitleOverride,
    bool useBackButton = false,
    bool homeManaged = true,
  }) {
    return Navigator.of(context).push(
      PageRouteBuilder(
        opaque: false,
        barrierColor: Colors.black54,
        pageBuilder: (context, animation, secondaryAnimation) {
          return RhythmServerSettingsScreen(
            hub: hub,
            headerTitleOverride: headerTitleOverride,
            useBackButton: useBackButton,
            homeManaged: homeManaged,
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
  State<RhythmServerSettingsScreen> createState() =>
      _RhythmServerSettingsScreenState();
}

class _RhythmServerSettingsScreenState extends State<RhythmServerSettingsScreen>
    with SingleTickerProviderStateMixin {
  final OtaService _otaService = OtaService();

  bool _isOnline = false;
  bool _isRemovingFromHome = false;
  bool _isRebooting = false;
  bool _isFactoryResetting = false;
  bool _isRefreshing = false;
  bool _isSubmittingDebugBundle = false;
  bool _isChangingWifi = false;
  OtaState? _lastHandledOtaState;
  String? _detachedServerPlatformType;
  String? _detachedServerPlatformContext;

  late AnimationController _glowController;
  late Animation<double> _glowAnimation;

  static const _teal = Color(0xFF00BCD4);
  static const _tealDeep = Color(0xFF0097A7);

  // ─── Server-type-aware computed getters ────────────────────

  String get _serverPlatformType => widget.homeManaged
      ? context.read<ServerSyncProvider>().serverPlatformType
      : _detachedServerPlatformType ?? 'desktop';
  String get _serverContext => widget.homeManaged
      ? context.read<ServerSyncProvider>().serverPlatformContext
      : _detachedServerPlatformContext ?? 'server';
  String get _serverVersion {
    final otaVersion = _otaService.currentVersion;
    if (otaVersion != '0.0.0') return otaVersion;
    if (!widget.homeManaged) return '0.0.0';
    return context.read<ServerSyncProvider>().firmwareVersion;
  }

  bool get _isBridge =>
      _serverContext == 'bridge' ||
      _serverContext == 'embedded' ||
      _serverContext == 'rpiz';
  bool get _isHaAddon => _serverContext == 'ha_addon';
  bool get _supportsDebugBundle => true;
  bool get _supportsWifiChange => _serverContext == 'rpiz';

  String get _headerTitle =>
      widget.headerTitleOverride ??
      switch (_serverContext) {
        'ha_addon' => 'Rhythm Add-on',
        'server' || 'rpiz' => 'RhythmOS Server',
        _ => 'LightBox',
      };

  IconData get _heroIcon => switch (_serverContext) {
        'ha_addon' => Icons.home_outlined,
        'server' || 'rpiz' => Icons.dns_outlined,
        _ => Icons.developer_board,
      };

  Hub get _currentHub {
    if (!widget.homeManaged) return widget.hub;
    final hubs = context.read<HomeProvider>().currentHomeHubs;
    return hubs
        .where((hub) => hub.id == widget.hub.id)
        .cast<Hub?>()
        .firstWhere((hub) => hub != null, orElse: () => widget.hub)!;
  }

  @override
  void initState() {
    super.initState();

    _glowController = AnimationController(
      duration: const Duration(milliseconds: 2000),
      vsync: this,
    )..repeat(reverse: true);

    _glowAnimation = Tween<double>(begin: 0.3, end: 0.6).animate(
      CurvedAnimation(parent: _glowController, curve: Curves.easeInOut),
    );

    _otaService.addListener(_onOtaStateChanged);
    _checkHealth();
    unawaited(_loadOtaSupport());

    AnalyticsService().logScreenView('rhythmserver_settings');
  }

  Future<ResolvedServerEndpoint> _resolveServerEndpoint() {
    if (!widget.homeManaged) {
      return Future.value(ServerEndpointResolver.local(_currentHub));
    }
    return ServerEndpointResolver.resolve(
      _currentHub,
      syncProvider: context.read<ServerSyncProvider>(),
    );
  }

  Future<RhythmDiagnosticsApi> _diagnosticsClient() async {
    final resolved = await _resolveServerEndpoint();
    return resolved.diagnosticsApi();
  }

  @override
  void dispose() {
    _otaService.removeListener(_onOtaStateChanged);
    _otaService.dispose();
    _glowController.dispose();
    super.dispose();
  }

  void _onOtaStateChanged() {
    if (!mounted) return;

    if (_otaService.state == _lastHandledOtaState) {
      setState(() {});
      return;
    }

    _lastHandledOtaState = _otaService.state;

    if (_otaService.state == OtaState.complete) {
      AnalyticsService().logOtaUpdateCompleted(
        _otaService.currentVersion,
      );
      // Force a full reconnect so the sync provider picks up the new
      // firmware version from GET /api/state (the poll endpoint doesn't
      // include version info).
      if (widget.homeManaged) {
        context.read<ServerSyncProvider>().connection.reconnect();
      }
    } else if (_otaService.state == OtaState.error) {
      AnalyticsService().logOtaUpdateFailed(
        _otaService.errorMessage ?? 'Unknown error',
      );
    }

    setState(() {});
  }

  Future<void> _loadOtaSupport() async {
    final syncProvider = context.read<ServerSyncProvider>();
    final resolved = await _resolveServerEndpoint();
    String? fallbackCurrentVersion =
        widget.homeManaged ? syncProvider.firmwareVersion : null;
    String? fallbackPlatformType =
        widget.homeManaged ? syncProvider.serverPlatformType : null;
    String? fallbackPlatformContext =
        widget.homeManaged ? syncProvider.serverPlatformContext : null;

    if (!widget.homeManaged) {
      try {
        final state = await RhythmConfigApi(
          baseUrl: resolved.endpoint.baseUrl,
          authToken: resolved.hub.token,
        ).getState().timeout(const Duration(seconds: 10));
        fallbackCurrentVersion = state.version;
        fallbackPlatformType = state.platformType;
        fallbackPlatformContext = state.platformContext;
        if (mounted) {
          setState(() {
            _detachedServerPlatformType = state.platformType;
            _detachedServerPlatformContext = state.platformContext;
          });
        }
      } catch (error) {
        debugPrint('Detached server metadata load failed: $error');
      }
    }

    await _otaService.initialize(
      host: resolved.endpoint.host,
      port: resolved.endpoint.port,
      useSsl: resolved.endpoint.useSsl,
      fallbackCurrentVersion: fallbackCurrentVersion,
      fallbackPlatformType: fallbackPlatformType,
      fallbackPlatformContext: fallbackPlatformContext,
      resetCheckStateOnInitialize: true,
      authToken: resolved.hub.token,
    );
  }

  Future<void> _checkHealth() async {
    final client = await _diagnosticsClient();
    final online = await client.healthCheck();
    if (mounted) {
      setState(() {
        _isOnline = online;
      });
    }
  }

  Future<void> _handleRefresh() async {
    if (_isRefreshing) return;
    setState(() {
      _isRefreshing = true;
    });

    // Refresh everything in parallel: health, server state, matter devices.
    await Future.wait([
      _checkHealth(),
      _loadOtaSupport(),
      if (widget.homeManaged)
        () async {
          // Force SSE reconnect to re-fetch /api/state (rooms, hubs, settings).
          final sync = context.read<ServerSyncProvider>();
          sync.connection.reconnect();
        }(),
    ]);

    if (mounted) {
      setState(() => _isRefreshing = false);
    }
  }

  Future<void> _handleRemoveFromHome() async {
    if (!widget.homeManaged) return;

    final confirmed = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: Text(
          'Remove $_headerTitle from Home',
          style: const TextStyle(color: CelestialColors.textPrimary),
        ),
        content: Text(
          'This removes the $_headerTitle from this Home and account restore. It does not factory reset the device. You can pair it again from Settings.',
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
              'Remove from Home',
              style: TextStyle(color: Colors.red),
            ),
          ),
        ],
      ),
    );

    if (confirmed != true || !mounted) return;

    AnalyticsService().logRhythmServerReset(wasOnline: _isOnline);
    setState(() => _isRemovingFromHome = true);

    // Clear all rooms first (server was source of truth)
    if (mounted) {
      await context.read<RoomProvider>().clearAllRooms();
    }

    // Disconnect the shared HTTP client via the sync provider
    if (mounted) {
      context.read<ServerSyncProvider>().connection.disconnect();
    }

    // Delete hub from local storage + Supabase
    if (mounted) {
      await context.read<HomeProvider>().deleteHub(widget.hub.id);
    }

    // Pop back to root
    if (mounted) {
      Navigator.of(context).popUntil((route) => route.isFirst);
    }
  }

  // ─── Connection status helpers ───────────────────────────

  String _connectionStatusText(RhythmConnectionState state) {
    switch (state) {
      case RhythmConnectionState.connected:
        return 'Connected';
      case RhythmConnectionState.connecting:
        return 'Connecting';
      case RhythmConnectionState.reconnecting:
        return 'Reconnecting';
      case RhythmConnectionState.disconnected:
        return 'Disconnected';
    }
  }

  Color _connectionStatusColor(RhythmConnectionState state) {
    switch (state) {
      case RhythmConnectionState.connected:
        return const Color(0xFF22C55E);
      case RhythmConnectionState.connecting:
      case RhythmConnectionState.reconnecting:
        return Colors.amber;
      case RhythmConnectionState.disconnected:
        return CelestialColors.textSecondary;
    }
  }

  // ─── Build ────────────────────────────────────────────────

  @override
  Widget build(BuildContext context) {
    // Watch ServerSyncProvider for rebuilds (hello, connection state, triage).
    // Read the RhythmConnection for SSE debug info (not a ChangeNotifier,
    // but rebuilt via ServerSyncProvider which listens to its streams).
    context.watch<ServerSyncProvider>();
    final http = context.read<RhythmConnection>();

    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            _buildHeader(),
            Expanded(
              child: RefreshIndicator(
                onRefresh: _handleRefresh,
                color: _teal,
                backgroundColor: CelestialColors.backgroundCard,
                child: SingleChildScrollView(
                  physics: const AlwaysScrollableScrollPhysics(),
                  padding: const EdgeInsets.symmetric(horizontal: 24),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      const SizedBox(height: 8),
                      _buildHeroSection(http),
                      const SizedBox(height: 24),
                      if (widget.homeManaged) _buildSettingsNavigationSection(),
                      if (_supportsWifiChange) ...[
                        const SizedBox(height: 16),
                        _buildNetworkSection(),
                      ],
                      const SizedBox(height: 16),
                      _buildVersionSection(),
                      if (_supportsDebugBundle) ...[
                        const SizedBox(height: 16),
                        _buildDebugSection(),
                      ],
                      const SizedBox(height: 24),
                      if (!_isHaAddon) ...[
                        _buildRebootButton(),
                        const SizedBox(height: 12),
                      ],
                      if (widget.homeManaged) ...[
                        _buildRemoveFromHomeButton(),
                        const SizedBox(height: 12),
                        _buildFactoryResetButton(),
                      ],
                      const SizedBox(height: 40),
                    ],
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildHeader() {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
      child: Row(
        children: [
          GestureDetector(
            onTap: () => Navigator.of(context).pop(),
            child: Container(
              width: 40,
              height: 40,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: _teal.withValues(alpha: 0.15),
                border: Border.all(
                  color: _teal.withValues(alpha: 0.3),
                  width: 1,
                ),
              ),
              child: Icon(
                widget.useBackButton ? Icons.chevron_left : Icons.close,
                color: _teal,
                size: widget.useBackButton ? 24 : 20,
              ),
            ),
          ),
          Expanded(
            child: Text(
              _headerTitle,
              textAlign: TextAlign.center,
              style: const TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 18,
                fontWeight: FontWeight.w600,
                letterSpacing: 0.3,
              ),
            ),
          ),
          const SizedBox(width: 40),
        ],
      ),
    );
  }

  Widget _buildHeroSection(RhythmConnection http) {
    final connState = widget.homeManaged
        ? http.connectionState
        : _isOnline
            ? RhythmConnectionState.connected
            : RhythmConnectionState.disconnected;
    final fullyOffline =
        widget.homeManaged ? !http.connected && !_isOnline : !_isOnline;
    final statusText = _connectionStatusText(connState);
    final statusColor = _connectionStatusColor(connState);
    final isLive = connState == RhythmConnectionState.connected;
    final isWorking = connState == RhythmConnectionState.connecting ||
        connState == RhythmConnectionState.reconnecting;

    return AnimatedBuilder(
      animation: _glowAnimation,
      builder: (context, child) {
        // Dim static glow when fully offline, otherwise breathe
        final glowIntensity = fullyOffline ? 0.15 : _glowAnimation.value;

        return Container(
          padding: const EdgeInsets.fromLTRB(24, 28, 24, 24),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(24),
            gradient: RadialGradient(
              center: Alignment.center,
              radius: 1.2,
              colors: [
                _teal.withValues(alpha: glowIntensity * 0.15),
                CelestialColors.backgroundCard,
              ],
            ),
            border: Border.all(
              color: _teal.withValues(alpha: fullyOffline ? 0.1 : 0.2),
              width: 1,
            ),
          ),
          child: Column(
            children: [
              SizedBox(
                width: 96,
                height: 96,
                child: Stack(
                  alignment: Alignment.center,
                  children: [
                    // Outer halo ring tinted by connection state
                    Container(
                      width: 96,
                      height: 96,
                      decoration: BoxDecoration(
                        shape: BoxShape.circle,
                        border: Border.all(
                          color: statusColor.withValues(
                              alpha: isWorking ? 0.55 : (isLive ? 0.45 : 0.2)),
                          width: 1.2,
                        ),
                      ),
                    ),
                    // Hero disc
                    Container(
                      width: 72,
                      height: 72,
                      decoration: BoxDecoration(
                        shape: BoxShape.circle,
                        gradient: LinearGradient(
                          begin: Alignment.topLeft,
                          end: Alignment.bottomRight,
                          colors: fullyOffline
                              ? [
                                  _teal.withValues(alpha: 0.3),
                                  _tealDeep.withValues(alpha: 0.2),
                                ]
                              : [_teal, _tealDeep],
                        ),
                        boxShadow: [
                          BoxShadow(
                            color: _teal.withValues(alpha: glowIntensity),
                            blurRadius: 24,
                            spreadRadius: 2,
                          ),
                        ],
                      ),
                      child: Icon(
                        _heroIcon,
                        color: fullyOffline
                            ? Colors.white.withValues(alpha: 0.5)
                            : Colors.white,
                        size: 36,
                      ),
                    ),
                    // Status pip — anchored to the halo ring
                    Positioned(
                      top: 4,
                      right: 4,
                      child: Container(
                        width: 14,
                        height: 14,
                        decoration: BoxDecoration(
                          shape: BoxShape.circle,
                          color: statusColor,
                          border: Border.all(
                            color: CelestialColors.backgroundCard,
                            width: 2,
                          ),
                          boxShadow: isLive || isWorking
                              ? [
                                  BoxShadow(
                                    color: statusColor.withValues(
                                        alpha: isWorking
                                            ? _glowAnimation.value
                                            : 0.6),
                                    blurRadius: 8,
                                    spreadRadius: 1,
                                  ),
                                ]
                              : null,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
              const SizedBox(height: 18),
              Text(
                widget.hub.name,
                style: TextStyle(
                  color: fullyOffline
                      ? CelestialColors.textPrimary.withValues(alpha: 0.6)
                      : CelestialColors.textPrimary,
                  fontSize: 16,
                  fontWeight: FontWeight.w500,
                ),
              ),
              const SizedBox(height: 4),
              Text(
                _currentHub.endpoint.host,
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.7),
                  fontSize: 12,
                  fontFamily: 'monospace',
                  letterSpacing: 0.3,
                ),
              ),
              const SizedBox(height: 12),
              _buildHeroStatusPill(
                statusText: statusText,
                statusColor: statusColor,
                isWorking: isWorking,
              ),
            ],
          ),
        );
      },
    );
  }

  Widget _buildHeroStatusPill({
    required String statusText,
    required Color statusColor,
    required bool isWorking,
  }) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(999),
        color: statusColor.withValues(alpha: 0.10),
        border: Border.all(
          color: statusColor.withValues(alpha: 0.30),
          width: 1,
        ),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (isWorking)
            SizedBox(
              width: 10,
              height: 10,
              child: CircularProgressIndicator(
                strokeWidth: 1.5,
                valueColor: AlwaysStoppedAnimation(statusColor),
              ),
            )
          else
            Container(
              width: 6,
              height: 6,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: statusColor,
                boxShadow: [
                  BoxShadow(
                    color: statusColor.withValues(alpha: 0.5),
                    blurRadius: 4,
                    spreadRadius: 0.5,
                  ),
                ],
              ),
            ),
          const SizedBox(width: 8),
          Text(
            statusText,
            style: TextStyle(
              color: statusColor,
              fontSize: 12,
              fontWeight: FontWeight.w600,
              letterSpacing: 0.4,
            ),
          ),
        ],
      ),
    );
  }

  // ─── Sections ──────────────────────────────────────────────

  Widget _buildSettingsNavigationSection() {
    return _buildSection(
      title: 'SETTINGS',
      children: [
        GestureDetector(
          onTap: _showServerSettings,
          behavior: HitTestBehavior.opaque,
          child: Container(
            decoration: BoxDecoration(
              color: CelestialColors.backgroundCard,
              borderRadius: BorderRadius.circular(14),
              border: Border.all(
                color: CelestialColors.orbitRing.withValues(alpha: 0.5),
              ),
            ),
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
              child: Row(
                children: [
                  Container(
                    width: 40,
                    height: 40,
                    decoration: BoxDecoration(
                      shape: BoxShape.circle,
                      color: _teal.withValues(alpha: 0.16),
                    ),
                    child: const Icon(
                      Icons.settings_outlined,
                      color: _teal,
                      size: 20,
                    ),
                  ),
                  const SizedBox(width: 14),
                  const Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          'Settings',
                          style: TextStyle(
                            color: CelestialColors.textPrimary,
                            fontSize: 15,
                            fontWeight: FontWeight.w600,
                          ),
                        ),
                        SizedBox(height: 4),
                        Text(
                          'Security and server controls',
                          style: TextStyle(
                            color: CelestialColors.textSecondary,
                            fontSize: 13,
                          ),
                        ),
                      ],
                    ),
                  ),
                  const SizedBox(width: 12),
                  Icon(
                    Icons.chevron_right,
                    color: CelestialColors.textSecondary.withValues(alpha: 0.5),
                    size: 22,
                  ),
                ],
              ),
            ),
          ),
        ),
      ],
    );
  }

  Future<void> _showServerSettings() {
    return Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) => const _RhythmServerAdvancedSettingsScreen(),
      ),
    );
  }

  Widget _buildNetworkSection() {
    final syncProvider = context.watch<ServerSyncProvider>();
    final connected = widget.homeManaged
        ? syncProvider.connectionState == RhythmConnectionState.connected
        : _isOnline;
    final canChange = !_isChangingWifi && (connected || _isOnline);
    final subtitle = _isChangingWifi
        ? 'Saving new network...'
        : canChange
            ? 'Move this server to another network'
            : 'Unavailable while offline';
    final accent = canChange ? _teal : CelestialColors.textSecondary;

    return _buildSection(
      title: 'NETWORK',
      children: [
        GestureDetector(
          onTap: canChange ? _handleChangeWifi : null,
          behavior: HitTestBehavior.opaque,
          child: AnimatedOpacity(
            duration: const Duration(milliseconds: 150),
            opacity: canChange || _isChangingWifi ? 1 : 0.55,
            child: Container(
              decoration: BoxDecoration(
                color: CelestialColors.backgroundCard,
                borderRadius: BorderRadius.circular(14),
                border: Border.all(
                  color: CelestialColors.orbitRing.withValues(alpha: 0.5),
                ),
              ),
              child: Padding(
                padding:
                    const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
                child: Row(
                  children: [
                    Container(
                      width: 40,
                      height: 40,
                      decoration: BoxDecoration(
                        shape: BoxShape.circle,
                        color: accent.withValues(alpha: 0.16),
                      ),
                      child: Icon(
                        Icons.wifi_rounded,
                        color: accent,
                        size: 20,
                      ),
                    ),
                    const SizedBox(width: 14),
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          const Text(
                            'Change Wi-Fi',
                            style: TextStyle(
                              color: CelestialColors.textPrimary,
                              fontSize: 15,
                              fontWeight: FontWeight.w600,
                            ),
                          ),
                          const SizedBox(height: 4),
                          Text(
                            subtitle,
                            style: const TextStyle(
                              color: CelestialColors.textSecondary,
                              fontSize: 13,
                            ),
                          ),
                        ],
                      ),
                    ),
                    const SizedBox(width: 12),
                    if (_isChangingWifi)
                      SizedBox(
                        width: 20,
                        height: 20,
                        child: CircularProgressIndicator(
                          strokeWidth: 2,
                          valueColor: AlwaysStoppedAnimation(accent),
                        ),
                      )
                    else
                      Icon(
                        Icons.chevron_right,
                        color: CelestialColors.textSecondary
                            .withValues(alpha: 0.5),
                        size: 22,
                      ),
                  ],
                ),
              ),
            ),
          ),
        ),
      ],
    );
  }

  Future<void> _handleChangeWifi() async {
    if (_isChangingWifi) return;

    FocusManager.instance.primaryFocus?.unfocus();
    final credentials = await _showWifiCredentialsDialog();
    if (!mounted) return;
    FocusManager.instance.primaryFocus?.unfocus();
    if (credentials == null) {
      if (_isChangingWifi) setState(() => _isChangingWifi = false);
      return;
    }

    setState(() => _isChangingWifi = true);
    try {
      final client = await _diagnosticsClient();
      final result = await client.changeWifi(
        ssid: credentials.ssid,
        password: credentials.password,
      );
      if (!mounted) return;

      if (result.accepted) {
        _showSnackBar('Wi-Fi change started. Reconnecting shortly.');
        unawaited(_refreshAfterWifiChange());
      } else {
        _showSnackBar(result.error ?? 'Could not change Wi-Fi.',
            backgroundColor: Colors.red.shade400);
      }
    } catch (error, stackTrace) {
      debugPrint('RhythmServerSettings: Wi-Fi change failed: $error');
      debugPrint('$stackTrace');
      if (mounted) {
        _showSnackBar(
          'Could not change Wi-Fi.',
          backgroundColor: Colors.red.shade400,
        );
      }
    } finally {
      if (mounted) {
        setState(() => _isChangingWifi = false);
      }
    }
  }

  Future<({String ssid, String password})?> _showWifiCredentialsDialog() {
    final ssidController = TextEditingController();
    final passwordController = TextEditingController();

    return showDialog<({String ssid, String password})>(
      context: context,
      builder: (ctx) {
        return StatefulBuilder(
          builder: (ctx, setDialogState) {
            final canSubmit = ssidController.text.trim().isNotEmpty;

            return AlertDialog(
              backgroundColor: CelestialColors.backgroundCard,
              title: const Text(
                'Change Wi-Fi',
                style: TextStyle(color: CelestialColors.textPrimary),
              ),
              content: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  TextField(
                    controller: ssidController,
                    autofocus: true,
                    textInputAction: TextInputAction.next,
                    onChanged: (_) => setDialogState(() {}),
                    style: const TextStyle(color: CelestialColors.textPrimary),
                    decoration: const InputDecoration(
                      labelText: 'Network name',
                      prefixIcon: Icon(Icons.wifi_rounded),
                    ),
                  ),
                  const SizedBox(height: 12),
                  TextField(
                    controller: passwordController,
                    obscureText: true,
                    textInputAction: TextInputAction.done,
                    onSubmitted: (_) {
                      if (!canSubmit) return;
                      Navigator.of(
                        ctx,
                        rootNavigator: true,
                      ).pop((
                        ssid: ssidController.text.trim(),
                        password: passwordController.text,
                      ));
                    },
                    style: const TextStyle(color: CelestialColors.textPrimary),
                    decoration: const InputDecoration(
                      labelText: 'Password',
                      prefixIcon: Icon(Icons.lock_outline_rounded),
                    ),
                  ),
                ],
              ),
              actions: [
                TextButton(
                  onPressed: () => Navigator.of(
                    ctx,
                    rootNavigator: true,
                  ).pop(),
                  child: Text(
                    'Cancel',
                    style: TextStyle(color: CelestialColors.textSecondary),
                  ),
                ),
                TextButton(
                  onPressed: canSubmit
                      ? () => Navigator.of(
                            ctx,
                            rootNavigator: true,
                          ).pop((
                            ssid: ssidController.text.trim(),
                            password: passwordController.text,
                          ))
                      : null,
                  child: const Text('Change'),
                ),
              ],
            );
          },
        );
      },
    ).whenComplete(() {
      ssidController.dispose();
      passwordController.dispose();
    });
  }

  Future<void> _refreshAfterWifiChange() async {
    await Future<void>.delayed(const Duration(seconds: 35));
    if (!mounted) return;
    await _checkHealth();
    if (!mounted) return;
    if (widget.homeManaged) {
      context.read<ServerSyncProvider>().connection.reconnect();
    }
  }

  void _showSnackBar(String message, {Color? backgroundColor}) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(message),
        behavior: SnackBarBehavior.floating,
        backgroundColor: backgroundColor,
      ),
    );
  }

  Widget _buildVersionSection() {
    final syncProvider = context.watch<ServerSyncProvider>();
    final currentVersion = _serverVersion;
    final showOtaControls =
        _otaService.isLoadingSupport || _otaService.showUpdateUi;
    final showAutoUpdateToggle =
        widget.homeManaged && !_isHaAddon && _otaService.isSelfPull;
    final autoUpdateEnabled = widget.homeManaged && syncProvider.autoUpdate;
    final allowManualUpdate = !autoUpdateEnabled || !showAutoUpdateToggle;

    return _buildSection(
      title: _isBridge ? 'FIRMWARE' : 'VERSION',
      children: [
        Container(
          decoration: BoxDecoration(
            color: CelestialColors.backgroundCard,
            borderRadius: BorderRadius.circular(14),
            border: Border.all(
              color: CelestialColors.orbitRing.withValues(alpha: 0.5),
            ),
          ),
          child: Column(
            children: [
              // Current version row
              Padding(
                padding:
                    const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
                child: Row(
                  mainAxisAlignment: MainAxisAlignment.spaceBetween,
                  children: [
                    Text(
                      'Current Version',
                      style: TextStyle(
                        color: CelestialColors.textSecondary
                            .withValues(alpha: 0.8),
                        fontSize: 14,
                      ),
                    ),
                    Text(
                      currentVersion == '0.0.0'
                          ? 'Unknown'
                          : _formatOtaVersion(currentVersion),
                      style: const TextStyle(
                        color: CelestialColors.textPrimary,
                        fontSize: 14,
                        fontWeight: FontWeight.w500,
                        fontFamily: 'monospace',
                      ),
                    ),
                  ],
                ),
              ),

              if (_otaService.lastRollback != null)
                Padding(
                  padding: const EdgeInsets.fromLTRB(16, 0, 16, 12),
                  child: Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Icon(
                        Icons.history_rounded,
                        color: CelestialColors.textSecondary
                            .withValues(alpha: 0.5),
                        size: 14,
                      ),
                      const SizedBox(width: 6),
                      Expanded(
                        child: Text(
                          'Update to ${_formatOtaVersion(_otaService.lastRollback!.version)} '
                          'was rolled back after a failed install.',
                          style: TextStyle(
                            color: CelestialColors.textSecondary
                                .withValues(alpha: 0.6),
                            fontSize: 12,
                          ),
                        ),
                      ),
                    ],
                  ),
                ),

              if (showAutoUpdateToggle) _buildAutoUpdateRow(autoUpdateEnabled),

              if (showOtaControls && allowManualUpdate)
                ..._buildOtaStateContent(currentVersion),

              // HA addon managed note
              if (_isHaAddon)
                Padding(
                  padding: const EdgeInsets.fromLTRB(16, 0, 16, 12),
                  child: Row(
                    children: [
                      Icon(
                        Icons.info_outline_rounded,
                        color: CelestialColors.textSecondary
                            .withValues(alpha: 0.5),
                        size: 16,
                      ),
                      const SizedBox(width: 8),
                      Text(
                        'Updates managed by Home Assistant',
                        style: TextStyle(
                          color: CelestialColors.textSecondary
                              .withValues(alpha: 0.6),
                          fontSize: 13,
                        ),
                      ),
                    ],
                  ),
                ),
            ],
          ),
        ),
      ],
    );
  }

  Widget _buildDebugSection() {
    final subtitle = _isSubmittingDebugBundle
        ? 'Generating logs and creating a bug report...'
        : 'Send logs and redacted state from this server to Rhythm support.';

    return _buildSection(
      title: 'DEBUG',
      children: [
        GestureDetector(
          onTap: _isSubmittingDebugBundle ? null : _submitDebugBundle,
          behavior: HitTestBehavior.opaque,
          child: AnimatedOpacity(
            duration: const Duration(milliseconds: 150),
            opacity: _isSubmittingDebugBundle ? 0.7 : 1,
            child: Container(
              decoration: BoxDecoration(
                color: CelestialColors.backgroundCard,
                borderRadius: BorderRadius.circular(14),
                border: Border.all(
                  color: CelestialColors.orbitRing.withValues(alpha: 0.5),
                ),
              ),
              child: Padding(
                padding: const EdgeInsets.all(16),
                child: Row(
                  children: [
                    Container(
                      width: 40,
                      height: 40,
                      decoration: BoxDecoration(
                        color: _teal.withValues(alpha: 0.16),
                        shape: BoxShape.circle,
                      ),
                      child: const Icon(
                        Icons.cloud_upload_outlined,
                        color: _teal,
                        size: 20,
                      ),
                    ),
                    const SizedBox(width: 14),
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          const Text(
                            'Report Bug',
                            style: TextStyle(
                              color: CelestialColors.textPrimary,
                              fontSize: 15,
                              fontWeight: FontWeight.w600,
                            ),
                          ),
                          const SizedBox(height: 4),
                          Text(
                            subtitle,
                            style: TextStyle(
                              color: CelestialColors.textSecondary
                                  .withValues(alpha: 0.7),
                              fontSize: 13,
                              height: 1.3,
                            ),
                          ),
                        ],
                      ),
                    ),
                    const SizedBox(width: 12),
                    if (_isSubmittingDebugBundle)
                      const SizedBox(
                        width: 18,
                        height: 18,
                        child: CircularProgressIndicator(
                          strokeWidth: 2,
                          valueColor: AlwaysStoppedAnimation(_teal),
                        ),
                      )
                    else
                      Icon(
                        Icons.chevron_right,
                        color: CelestialColors.textSecondary
                            .withValues(alpha: 0.5),
                        size: 22,
                      ),
                  ],
                ),
              ),
            ),
          ),
        ),
      ],
    );
  }

  Future<void> _submitDebugBundle() async {
    if (_isSubmittingDebugBundle) return;
    setState(() => _isSubmittingDebugBundle = true);
    try {
      await showReportBugFlow(
        context,
        serverHub: _currentHub,
        localServerOnly: !widget.homeManaged,
        serverVersionOverride: widget.homeManaged ? null : _serverVersion,
        serverPlatformContextOverride:
            widget.homeManaged ? null : _serverContext,
      );
    } finally {
      if (mounted) {
        setState(() => _isSubmittingDebugBundle = false);
      }
    }
  }

  Widget _buildAutoUpdateRow(bool enabled) {
    return Column(
      children: [
        Divider(
          height: 1,
          color: CelestialColors.orbitRing.withValues(alpha: 0.3),
        ),
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 4),
          child: Row(
            children: [
              Expanded(
                child: Text(
                  'Auto Update',
                  style: TextStyle(
                    color: CelestialColors.textSecondary.withValues(alpha: 0.8),
                    fontSize: 14,
                  ),
                ),
              ),
              const SizedBox(width: 12),
              Switch.adaptive(
                value: enabled,
                activeTrackColor: _teal,
                onChanged: (next) {
                  unawaited(
                    context.read<ServerSyncProvider>().setAutoUpdate(next),
                  );
                },
              ),
            ],
          ),
        ),
      ],
    );
  }

  List<Widget> _buildOtaStateContent(String currentVersion) {
    if (_otaService.isLoadingSupport) {
      return [
        _buildOtaButton(
          label: 'Loading update support...',
          icon: null,
          isLoading: true,
        ),
      ];
    }

    switch (_otaService.state) {
      case OtaState.idle:
        return [
          _buildOtaButton(
            label: 'Check for Update',
            icon: Icons.system_update_outlined,
            onTap: () {
              AnalyticsService().logOtaCheck(currentVersion);
              _otaService.checkForUpdate(currentVersion);
            },
          ),
        ];

      case OtaState.checking:
        return [
          _buildOtaButton(
            label: 'Checking...',
            icon: null,
            isLoading: true,
          ),
        ];

      case OtaState.upToDate:
        return [
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 0, 16, 12),
            child: Row(
              children: [
                const Icon(Icons.check_circle_outline,
                    color: Color(0xFF22C55E), size: 18),
                const SizedBox(width: 8),
                Text(
                  'Up to date',
                  style: TextStyle(
                    color: const Color(0xFF22C55E),
                    fontSize: 14,
                    fontWeight: FontWeight.w500,
                  ),
                ),
              ],
            ),
          ),
        ];

      case OtaState.available:
        final release = _otaService.availableRelease!;
        final targetVersionLabel = _formatOtaVersion(release.version);
        final updateMessage =
            release.updateReason == OtaUpdateReason.componentDrift
                ? 'A repair bundle is ready to install.'
                : 'A new update is ready to install.';
        return [
          Divider(
            height: 1,
            color: CelestialColors.orbitRing.withValues(alpha: 0.3),
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 12, 16, 8),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  'Update available',
                  style: TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 14,
                    fontWeight: FontWeight.w600,
                  ),
                ),
                const SizedBox(height: 4),
                Text(
                  updateMessage,
                  style: TextStyle(
                    color: CelestialColors.textSecondary.withValues(alpha: 0.7),
                    fontSize: 13,
                  ),
                ),
                if (_otaService.availableUpdateWasRolledBack) ...[
                  const SizedBox(height: 8),
                  Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      const Icon(
                        Icons.history_rounded,
                        color: Colors.amber,
                        size: 14,
                      ),
                      const SizedBox(width: 6),
                      Expanded(
                        child: Text(
                          'This version was rolled back after a failed '
                          'install on this device. Installing it again may '
                          'fail the same way.',
                          style: TextStyle(
                            color: Colors.amber.withValues(alpha: 0.9),
                            fontSize: 12,
                          ),
                        ),
                      ),
                    ],
                  ),
                ],
                const SizedBox(height: 10),
                Row(
                  mainAxisAlignment: MainAxisAlignment.spaceBetween,
                  children: [
                    Text(
                      'Updating To',
                      style: TextStyle(
                        color: CelestialColors.textSecondary
                            .withValues(alpha: 0.75),
                        fontSize: 12,
                        fontWeight: FontWeight.w500,
                        letterSpacing: 0.3,
                      ),
                    ),
                    Text(
                      targetVersionLabel,
                      style: const TextStyle(
                        color: _teal,
                        fontSize: 13,
                        fontWeight: FontWeight.w600,
                        fontFamily: 'monospace',
                      ),
                    ),
                  ],
                ),
              ],
            ),
          ),
          _buildOtaButton(
            label: 'Install Update',
            icon: Icons.download_rounded,
            onTap: () => unawaited(
              _startOtaUpdate(currentVersion, release.version),
            ),
          ),
        ];

      case OtaState.downloading:
      case OtaState.uploading:
        final pct = _otaService.progress;
        final label = _otaService.state == OtaState.downloading
            ? 'Downloading update...'
            : 'Installing update...';
        return [
          Divider(
            height: 1,
            color: CelestialColors.orbitRing.withValues(alpha: 0.3),
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 12, 16, 4),
            child: Row(
              mainAxisAlignment: MainAxisAlignment.spaceBetween,
              children: [
                Text(
                  label,
                  style: TextStyle(
                    color: CelestialColors.textSecondary.withValues(alpha: 0.8),
                    fontSize: 14,
                  ),
                ),
                if (pct != null)
                  Text(
                    '$pct%',
                    style: const TextStyle(
                      color: _teal,
                      fontSize: 14,
                      fontWeight: FontWeight.w600,
                      fontFamily: 'monospace',
                    ),
                  ),
              ],
            ),
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 4, 16, 16),
            child: ClipRRect(
              borderRadius: BorderRadius.circular(4),
              child: LinearProgressIndicator(
                value: pct != null ? pct / 100.0 : null,
                backgroundColor: _teal.withValues(alpha: 0.1),
                valueColor: const AlwaysStoppedAnimation(_teal),
                minHeight: 6,
              ),
            ),
          ),
        ];

      case OtaState.flashing:
        return [
          Divider(
            height: 1,
            color: CelestialColors.orbitRing.withValues(alpha: 0.3),
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 12, 16, 4),
            child: Text(
              'Installing update...',
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.8),
                fontSize: 14,
              ),
            ),
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 4, 16, 16),
            child: ClipRRect(
              borderRadius: BorderRadius.circular(4),
              child: LinearProgressIndicator(
                backgroundColor: _teal.withValues(alpha: 0.1),
                valueColor: const AlwaysStoppedAnimation(_teal),
                minHeight: 6,
              ),
            ),
          ),
        ];

      case OtaState.rebooting:
        return [
          Divider(
            height: 1,
            color: CelestialColors.orbitRing.withValues(alpha: 0.3),
          ),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 16),
            child: Row(
              children: [
                SizedBox(
                  width: 16,
                  height: 16,
                  child: CircularProgressIndicator(
                    strokeWidth: 2,
                    valueColor: AlwaysStoppedAnimation(_teal),
                  ),
                ),
                const SizedBox(width: 12),
                Text(
                  'Restarting device...',
                  style: TextStyle(
                    color: CelestialColors.textSecondary.withValues(alpha: 0.8),
                    fontSize: 14,
                  ),
                ),
              ],
            ),
          ),
        ];

      case OtaState.complete:
        return [
          Divider(
            height: 1,
            color: CelestialColors.orbitRing.withValues(alpha: 0.3),
          ),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
            child: Row(
              children: [
                const Icon(Icons.check_circle,
                    color: Color(0xFF22C55E), size: 20),
                const SizedBox(width: 10),
                Expanded(
                  child: const Text(
                    'Update completed successfully.',
                    style: TextStyle(
                      color: Color(0xFF22C55E),
                      fontSize: 14,
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                ),
              ],
            ),
          ),
          _buildOtaButton(
            label: 'Done',
            icon: null,
            onTap: () => _otaService.reset(),
          ),
        ];

      case OtaState.error:
        return [
          Divider(
            height: 1,
            color: CelestialColors.orbitRing.withValues(alpha: 0.3),
          ),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Icon(Icons.error_outline, color: Colors.red.shade400, size: 18),
                const SizedBox(width: 8),
                Expanded(
                  child: Text(
                    'The update did not finish. Please try again.',
                    style: TextStyle(
                      color: Colors.red.shade400,
                      fontSize: 13,
                    ),
                  ),
                ),
              ],
            ),
          ),
          _buildOtaButton(
            label: 'Retry',
            icon: Icons.refresh_rounded,
            onTap: () {
              _otaService.reset();
              AnalyticsService().logOtaCheck(currentVersion);
              _otaService.checkForUpdate(currentVersion);
            },
          ),
        ];
    }
  }

  String _formatOtaVersion(String version) {
    return version.startsWith('v') || version.startsWith('V')
        ? version
        : 'v$version';
  }

  Future<void> _startOtaUpdate(
    String currentVersion,
    String targetVersion,
  ) async {
    final resolved = await _resolveServerEndpoint();
    if (!mounted) return;

    AnalyticsService().logOtaUpdateStarted(
      currentVersion,
      targetVersion,
    );
    unawaited(_otaService.startUpdate(
      resolved.endpoint.host,
      port: resolved.endpoint.port,
      useSsl: resolved.endpoint.useSsl,
      authToken: resolved.hub.token,
    ));
    OtaUpdateOverlay.show(
      context,
      otaService: _otaService,
      connection: widget.homeManaged
          ? context.read<ServerSyncProvider>().connection
          : null,
    );
  }

  Widget _buildOtaButton({
    required String label,
    IconData? icon,
    VoidCallback? onTap,
    bool isLoading = false,
  }) {
    return GestureDetector(
      onTap: onTap,
      child: Container(
        margin: const EdgeInsets.fromLTRB(12, 4, 12, 12),
        padding: const EdgeInsets.symmetric(vertical: 10),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(10),
          color: _teal.withValues(alpha: 0.08),
          border: Border.all(
            color: _teal.withValues(alpha: 0.2),
          ),
        ),
        child: Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            if (isLoading)
              SizedBox(
                width: 16,
                height: 16,
                child: CircularProgressIndicator(
                  strokeWidth: 2,
                  valueColor: AlwaysStoppedAnimation(_teal),
                ),
              )
            else if (icon != null)
              Icon(icon, color: _teal, size: 18),
            const SizedBox(width: 8),
            Text(
              label,
              style: const TextStyle(
                color: _teal,
                fontSize: 14,
                fontWeight: FontWeight.w500,
              ),
            ),
          ],
        ),
      ),
    );
  }

  static String _hubLabel(String type) => switch (type) {
        'hue' => 'Philips Hue',
        'homeassistant' || 'home_assistant' => 'Home Assistant',
        'matter' => 'Matter',
        _ => type,
      };

  static Color _hubColor(String type) => switch (type) {
        'hue' => const Color(0xFFFFB900),
        'homeassistant' || 'home_assistant' => const Color(0xFF42A5F5),
        'matter' => const Color(0xFF26A69A),
        _ => _teal,
      };

  static IconData _hubIcon(String type) => switch (type) {
        'hue' => Icons.lightbulb_outline,
        'homeassistant' || 'home_assistant' => Icons.home_outlined,
        'matter' => Icons.memory_outlined,
        _ => Icons.hub_outlined,
      };

  static bool _hubHasBetaBadge(String type) => switch (type) {
        'homeassistant' || 'home_assistant' || 'matter' => true,
        _ => false,
      };

  static const Color _connectedGreen = Color(0xFF22C55E);
  static const Color _warningAmber = Color(0xFFE8A54B);
  static const Color _errorRed = Color(0xFFEF4444);

  static RhythmHubInfo _parsedHubInfo(Map<String, dynamic> hubInfo) {
    return RhythmHubInfo.fromJson(hubInfo);
  }

  static RhythmHubStartupRetry? _startupRetry(Map<String, dynamic> hubInfo) {
    return _parsedHubInfo(hubInfo).startupRetry;
  }

  static bool _hubNeedsManualRetry(Map<String, dynamic> hubInfo) {
    return (hubInfo['connected'] as bool? ?? false) != true &&
        _startupRetry(hubInfo)?.status ==
            RhythmHubStartupRetryStatus.manualRetryRequired;
  }

  static bool _hubIsAutoRetrying(Map<String, dynamic> hubInfo) {
    return (hubInfo['connected'] as bool? ?? false) != true &&
        _startupRetry(hubInfo)?.status == RhythmHubStartupRetryStatus.scheduled;
  }

  static Color _hubConnectionColor(Map<String, dynamic> hubInfo) {
    if (hubInfo['connected'] as bool? ?? false) return _connectedGreen;
    if (_hubIsAutoRetrying(hubInfo)) return _warningAmber;
    return _errorRed;
  }

  static String _hubConnectionLabel(Map<String, dynamic> hubInfo) {
    if (hubInfo['connected'] as bool? ?? false) return 'Connected';
    if (_hubIsAutoRetrying(hubInfo)) return 'Retrying Automatically';
    if (_hubNeedsManualRetry(hubInfo)) return 'Retry Required';
    return 'Disconnected';
  }

  static String? _hubRetrySummary(Map<String, dynamic> hubInfo) {
    final retry = _startupRetry(hubInfo);
    if (retry == null) return null;
    if (retry.isManualRetryRequired) {
      return 'Automatic retries paused after 24 hours';
    }
    final nextRetryEpochMs = retry.nextRetryEpochMs;
    if (retry.isScheduled && nextRetryEpochMs != null) {
      return 'Next retry ${_formatRetryEta(nextRetryEpochMs)}';
    }
    if (retry.isScheduled) {
      return 'Stored credentials are retrying automatically';
    }
    return null;
  }

  static String _hubRowSubtitle(
    Map<String, dynamic> hubInfo,
    String deviceSummary,
  ) {
    final retrySummary = _hubRetrySummary(hubInfo);
    if (retrySummary != null) return retrySummary;
    return deviceSummary;
  }

  static String _formatRetryEta(int epochMs) {
    final remaining =
        DateTime.fromMillisecondsSinceEpoch(epochMs).difference(DateTime.now());
    if (remaining.inSeconds <= 0) return 'again shortly';
    if (remaining.inMinutes < 1) return 'in ${remaining.inSeconds}s';
    if (remaining.inHours < 1) return 'in ${remaining.inMinutes}m';
    return 'in ${remaining.inHours}h';
  }

  Future<void> _handleReboot() async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: Text(
          'Reboot $_headerTitle',
          style: const TextStyle(color: CelestialColors.textPrimary),
        ),
        content: Text(
          'This will restart your $_headerTitle. It should come back online within a few seconds.',
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
            child: Text(
              'Reboot',
              style: TextStyle(color: Colors.orange.shade400),
            ),
          ),
        ],
      ),
    );

    if (confirmed != true || !mounted) return;

    setState(() => _isRebooting = true);

    final client = await _diagnosticsClient();
    final dispatched = await client.reboot();

    if (!mounted) return;

    setState(() => _isRebooting = false);

    if (!dispatched) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(
          content: Text('Failed to send reboot command.'),
          behavior: SnackBarBehavior.floating,
        ),
      );
      return;
    }

    final cameBack = await _RebootOverlay.show(
      context,
      client: client,
      headerTitle: _headerTitle,
    );
    if (!mounted) return;

    if (cameBack) {
      if (widget.homeManaged) {
        unawaited(context.read<ServerSyncProvider>().connection.reconnect());
      }
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          content: Text('$_headerTitle is back online.'),
          behavior: SnackBarBehavior.floating,
        ),
      );
    }
  }

  Widget _buildRebootButton() {
    final buttonColor = _isOnline
        ? Colors.orange.shade400
        : CelestialColors.textSecondary.withValues(alpha: 0.4);

    return GestureDetector(
      onTap: (_isOnline && !_isRebooting) ? _handleReboot : null,
      child: Container(
        padding: const EdgeInsets.symmetric(vertical: 14),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(14),
          color: _isOnline
              ? Colors.orange.shade400.withValues(alpha: 0.08)
              : Colors.white.withValues(alpha: 0.02),
          border: Border.all(
            color: _isOnline
                ? Colors.orange.shade400.withValues(alpha: 0.25)
                : CelestialColors.textSecondary.withValues(alpha: 0.1),
          ),
        ),
        child: Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            if (_isRebooting)
              SizedBox(
                width: 18,
                height: 18,
                child: CircularProgressIndicator(
                  strokeWidth: 2,
                  valueColor: AlwaysStoppedAnimation(buttonColor),
                ),
              )
            else
              Icon(
                Icons.power_settings_new,
                color: buttonColor,
                size: 18,
              ),
            const SizedBox(width: 10),
            Text(
              _isRebooting ? 'Rebooting...' : 'Reboot $_headerTitle',
              style: TextStyle(
                color: buttonColor,
                fontSize: 15,
                fontWeight: FontWeight.w500,
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildRemoveFromHomeButton() {
    final buttonColor = Colors.red.shade400;

    return GestureDetector(
      onTap: _isRemovingFromHome ? null : _handleRemoveFromHome,
      child: Container(
        padding: const EdgeInsets.symmetric(vertical: 16),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(14),
          color: Colors.red.shade400.withValues(alpha: 0.1),
          border: Border.all(
            color: Colors.red.shade400.withValues(alpha: 0.3),
          ),
        ),
        child: Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            if (_isRemovingFromHome)
              SizedBox(
                width: 20,
                height: 20,
                child: CircularProgressIndicator(
                  strokeWidth: 2,
                  valueColor: AlwaysStoppedAnimation(buttonColor),
                ),
              )
            else
              Icon(
                Icons.link_off_rounded,
                color: buttonColor,
                size: 20,
              ),
            const SizedBox(width: 10),
            Text(
              _isRemovingFromHome ? 'Removing...' : 'Remove from Home',
              style: TextStyle(
                color: buttonColor,
                fontSize: 15,
                fontWeight: FontWeight.w600,
              ),
            ),
          ],
        ),
      ),
    );
  }

  // ─── Factory Reset ─────────────────────────────────────────

  Future<void> _handleFactoryReset() async {
    final hubName = widget.hub.name;

    // Step 1: explain what will happen.
    final acknowledged = await showDialog<bool>(
      context: context,
      barrierDismissible: false,
      builder: (ctx) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(20),
          side: BorderSide(color: Colors.red.shade400.withValues(alpha: 0.4)),
        ),
        titlePadding: const EdgeInsets.fromLTRB(24, 24, 24, 8),
        title: Row(
          children: [
            Container(
              width: 36,
              height: 36,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: Colors.red.shade400.withValues(alpha: 0.15),
                border: Border.all(
                    color: Colors.red.shade400.withValues(alpha: 0.4)),
              ),
              child: Icon(Icons.warning_amber_rounded,
                  color: Colors.red.shade400, size: 20),
            ),
            const SizedBox(width: 12),
            const Expanded(
              child: Text(
                'Factory Reset',
                style: TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 18,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
          ],
        ),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              'This will erase all settings on $hubName and clear its Wi-Fi credentials. The device will restart in setup mode and you’ll need to re-pair it from scratch.',
              style: const TextStyle(
                color: CelestialColors.textSecondary,
                fontSize: 14,
                height: 1.4,
              ),
            ),
            const SizedBox(height: 16),
            _buildResetBullet('All paired hubs will be removed'),
            _buildResetBullet('Wi-Fi credentials will be cleared'),
            _buildResetBullet('Device returns to BLE setup mode'),
          ],
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
            child: Text(
              'Continue',
              style: TextStyle(color: Colors.red.shade400),
            ),
          ),
        ],
      ),
    );

    if (acknowledged != true || !mounted) return;

    // Step 2: hard confirmation, type-to-confirm.
    final confirmed = await _showFactoryResetConfirm(hubName);
    if (confirmed != true || !mounted) return;

    AnalyticsService().logRhythmServerReset(wasOnline: _isOnline);
    setState(() => _isFactoryResetting = true);

    final client = await _diagnosticsClient();
    final success = await client.factoryReset(
      platformType: _serverPlatformType,
      platformContext: _serverContext,
    );
    if (!mounted) return;

    if (!success) {
      setState(() => _isFactoryResetting = false);
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          content: Text('Failed to reach $hubName for factory reset.'),
          behavior: SnackBarBehavior.floating,
          backgroundColor: Colors.red.shade400,
        ),
      );
      return;
    }

    if (widget.homeManaged) {
      // Tear down the local pairing — device is going away to provisioning mode.
      if (mounted) {
        await context.read<RoomProvider>().clearAllRooms();
      }
      if (mounted) {
        context.read<ServerSyncProvider>().connection.disconnect();
      }
      if (mounted) {
        await context.read<HomeProvider>().deleteHub(widget.hub.id);
      }
    }

    if (mounted) {
      Navigator.of(context).popUntil((route) => route.isFirst);
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          content: Text(
              '$hubName factory reset. It will reappear in setup mode shortly.'),
          behavior: SnackBarBehavior.floating,
        ),
      );
    }
  }

  Future<bool?> _showFactoryResetConfirm(String hubName) {
    final controller = TextEditingController();
    const phrase = 'RESET';
    return showDialog<bool>(
      context: context,
      barrierDismissible: false,
      builder: (ctx) => StatefulBuilder(
        builder: (ctx, setLocal) {
          final matches = controller.text.trim().toUpperCase() == phrase;
          return AlertDialog(
            backgroundColor: CelestialColors.backgroundCard,
            shape: RoundedRectangleBorder(
              borderRadius: BorderRadius.circular(20),
              side:
                  BorderSide(color: Colors.red.shade400.withValues(alpha: 0.4)),
            ),
            title: const Text(
              'Confirm factory reset',
              style: TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 17,
                fontWeight: FontWeight.w600,
              ),
            ),
            content: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  'Type RESET to wipe $hubName.',
                  style: const TextStyle(
                    color: CelestialColors.textSecondary,
                    fontSize: 14,
                  ),
                ),
                const SizedBox(height: 14),
                TextField(
                  controller: controller,
                  autofocus: true,
                  textCapitalization: TextCapitalization.characters,
                  inputFormatters: [
                    FilteringTextInputFormatter.allow(RegExp(r'[A-Za-z]')),
                    LengthLimitingTextInputFormatter(8),
                  ],
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontFamily: 'monospace',
                    fontSize: 16,
                    letterSpacing: 2,
                  ),
                  decoration: InputDecoration(
                    hintText: phrase,
                    hintStyle: TextStyle(
                      color:
                          CelestialColors.textSecondary.withValues(alpha: 0.4),
                      letterSpacing: 2,
                      fontFamily: 'monospace',
                    ),
                    filled: true,
                    fillColor: Colors.red.shade400.withValues(alpha: 0.06),
                    enabledBorder: OutlineInputBorder(
                      borderRadius: BorderRadius.circular(10),
                      borderSide: BorderSide(
                          color: Colors.red.shade400.withValues(alpha: 0.3)),
                    ),
                    focusedBorder: OutlineInputBorder(
                      borderRadius: BorderRadius.circular(10),
                      borderSide: BorderSide(
                          color: Colors.red.shade400.withValues(alpha: 0.7)),
                    ),
                  ),
                  onChanged: (_) => setLocal(() {}),
                ),
              ],
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
                onPressed: matches ? () => Navigator.of(ctx).pop(true) : null,
                child: Text(
                  'Erase device',
                  style: TextStyle(
                    color: matches
                        ? Colors.red.shade400
                        : CelestialColors.textSecondary.withValues(alpha: 0.4),
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
            ],
          );
        },
      ),
    );
  }

  Widget _buildResetBullet(String text) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 3),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.only(top: 6),
            child: Container(
              width: 4,
              height: 4,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: Colors.red.shade400.withValues(alpha: 0.7),
              ),
            ),
          ),
          const SizedBox(width: 10),
          Expanded(
            child: Text(
              text,
              style: TextStyle(
                color: CelestialColors.textPrimary.withValues(alpha: 0.85),
                fontSize: 13,
                height: 1.35,
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildFactoryResetButton() {
    final disabled = _isFactoryResetting || _isRemovingFromHome || _isRebooting;
    final color = Colors.red.shade400;

    return GestureDetector(
      onTap: disabled ? null : _handleFactoryReset,
      child: Container(
        padding: const EdgeInsets.symmetric(vertical: 16),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(14),
          gradient: LinearGradient(
            begin: Alignment.topLeft,
            end: Alignment.bottomRight,
            colors: [
              color.withValues(alpha: disabled ? 0.10 : 0.22),
              color.withValues(alpha: disabled ? 0.04 : 0.10),
            ],
          ),
          border: Border.all(
            color: color.withValues(alpha: disabled ? 0.25 : 0.55),
            width: 1.2,
          ),
          boxShadow: disabled
              ? null
              : [
                  BoxShadow(
                    color: color.withValues(alpha: 0.18),
                    blurRadius: 14,
                    spreadRadius: -2,
                    offset: const Offset(0, 4),
                  ),
                ],
        ),
        child: Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            if (_isFactoryResetting)
              SizedBox(
                width: 20,
                height: 20,
                child: CircularProgressIndicator(
                  strokeWidth: 2,
                  valueColor: AlwaysStoppedAnimation(color),
                ),
              )
            else
              Icon(Icons.restart_alt_rounded, color: color, size: 20),
            const SizedBox(width: 10),
            Text(
              _isFactoryResetting ? 'Resetting…' : 'Factory Reset Device',
              style: TextStyle(
                color: color,
                fontSize: 15,
                fontWeight: FontWeight.w700,
                letterSpacing: 0.4,
              ),
            ),
          ],
        ),
      ),
    );
  }

  // ─── Shared UI ─────────────────────────────────────────────

  Widget _buildSection({
    required String title,
    required List<Widget> children,
  }) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.only(left: 4, bottom: 10),
          child: Text(
            title,
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.6),
              fontSize: 13,
              fontWeight: FontWeight.w500,
              letterSpacing: 0.8,
            ),
          ),
        ),
        ...children,
      ],
    );
  }
}

class _RhythmServerAdvancedSettingsScreen extends StatefulWidget {
  const _RhythmServerAdvancedSettingsScreen();

  @override
  State<_RhythmServerAdvancedSettingsScreen> createState() =>
      _RhythmServerAdvancedSettingsScreenState();
}

class _RhythmServerAdvancedSettingsScreenState
    extends State<_RhythmServerAdvancedSettingsScreen> {
  static const _teal = Color(0xFF00BCD4);
  static const _enabledGreen = Color(0xFF22C55E);
  static const _disabledRed = Color(0xFFEF4444);

  bool _isTogglingRemoteAccess = false;

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            _buildHeader(),
            Expanded(
              child: SingleChildScrollView(
                padding: const EdgeInsets.symmetric(horizontal: 24),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    const SizedBox(height: 8),
                    _buildAutomaticLightingSection(),
                    const SizedBox(height: 40),
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildHeader() {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
      child: Row(
        children: [
          GestureDetector(
            onTap: () => Navigator.of(context).pop(),
            child: Container(
              width: 40,
              height: 40,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: _teal.withValues(alpha: 0.15),
                border: Border.all(
                  color: _teal.withValues(alpha: 0.3),
                  width: 1,
                ),
              ),
              child: const Icon(
                Icons.chevron_left,
                color: _teal,
                size: 24,
              ),
            ),
          ),
          const Expanded(
            child: Text(
              'Settings',
              textAlign: TextAlign.center,
              style: TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 18,
                fontWeight: FontWeight.w600,
                letterSpacing: 0.3,
              ),
            ),
          ),
          const SizedBox(width: 40),
        ],
      ),
    );
  }

  Widget _buildAutomaticLightingSection() {
    final syncProvider = context.watch<ServerSyncProvider>();
    final homeProvider = context.watch<HomeProvider>();
    final connected =
        syncProvider.connectionState == RhythmConnectionState.connected;
    final enabled = syncProvider.lightBreakerEnabled;
    final statusText = connected ? (enabled ? 'On' : 'Off') : 'Offline';
    final statusColor = !connected
        ? CelestialColors.textSecondary.withValues(alpha: 0.6)
        : enabled
            ? _enabledGreen
            : _disabledRed;
    final serverHub = homeProvider.activeServerHub;
    final canUseRemoteAccess = _canUseRemoteAccess();
    final remoteAccessEnabled = serverHub?.remoteEndpoint != null;
    final remoteAccessAvailable = serverHub != null && canUseRemoteAccess;
    final remoteAccessStatusText = _isTogglingRemoteAccess
        ? 'Updating...'
        : remoteAccessEnabled
            ? 'On'
            : remoteAccessAvailable
                ? 'Off'
                : 'Unavailable';
    final remoteAccessStatusColor = _isTogglingRemoteAccess
        ? Colors.amber
        : !remoteAccessAvailable
            ? CelestialColors.textSecondary.withValues(alpha: 0.6)
            : remoteAccessEnabled
                ? _enabledGreen
                : _disabledRed;

    return _buildSection(
      title: 'SERVER',
      children: [
        _buildSwitchRow(
          icon: Icons.wb_sunny_rounded,
          iconColor: statusColor,
          label: 'Automatic Lighting',
          tooltip:
              'When on, Rhythm adjusts your lights through the day and responds to your switches and motion. Turn off to pause all of it.',
          statusText: statusText,
          statusColor: statusColor,
          value: enabled,
          activeTrackColor: _enabledGreen,
          onChanged: connected
              ? (next) => unawaited(syncProvider.setLightBreakerEnabled(next))
              : null,
        ),
        const SizedBox(height: 10),
        _buildSwitchRow(
          icon: Icons.public_rounded,
          iconColor: remoteAccessStatusColor,
          label: 'Remote Access',
          tooltip:
              'Turn off to remove the current tunnel. Turn on again to provision a fresh tunnel.',
          statusText: remoteAccessStatusText,
          statusColor: remoteAccessStatusColor,
          value: remoteAccessEnabled,
          activeTrackColor: _enabledGreen,
          busy: _isTogglingRemoteAccess,
          onChanged: remoteAccessAvailable && !_isTogglingRemoteAccess
              ? (next) => unawaited(_setRemoteAccessEnabled(next))
              : null,
        ),
      ],
    );
  }

  bool _canUseRemoteAccess() {
    try {
      return RemoteAccessService.instance.canUseRemoteAccess;
    } catch (error) {
      debugPrint('Remote access unavailable: $error');
      return false;
    }
  }

  Future<void> _setRemoteAccessEnabled(bool enabled) async {
    if (_isTogglingRemoteAccess) return;

    final homeProvider = context.read<HomeProvider>();
    final syncProvider = context.read<ServerSyncProvider>();
    final home = homeProvider.currentHome;
    var serverHub = homeProvider.activeServerHub;
    if (home == null || serverHub == null) {
      _showSnackBar(
        'No active server is available.',
        backgroundColor: Colors.red.shade400,
      );
      return;
    }

    setState(() => _isTogglingRemoteAccess = true);
    try {
      final service = RemoteAccessService.instance;
      Hub updatedHub;
      var routeVerified = true;
      if (enabled) {
        var tokenHub = await service.ensureOwnerTokenForHub(serverHub);
        if (tokenHub.token != serverHub.token) {
          final saved = await homeProvider.updateHub(tokenHub);
          if (!saved) {
            throw StateError('Could not save the server owner token.');
          }
          tokenHub = homeProvider.activeServerHub ?? tokenHub;
        }

        final result = await service.enableForHub(
          tokenHub,
          home: home,
          explicitUserEnable: true,
        );
        updatedHub = result.updatedHub;
        routeVerified = result.routeVerified;
      } else {
        updatedHub = await service.disableForHub(serverHub, home: home);
      }

      final saved = await homeProvider.updateHub(
        updatedHub,
        clearCloudRemoteEndpoint: !enabled,
      );
      if (!saved) {
        throw StateError('Could not save the remote access setting.');
      }

      syncProvider.connectIfAvailable();
      if (enabled && !routeVerified) {
        service.scheduleAutoEnableForHub(
          home: home,
          serverHub: updatedHub,
          saveHub: homeProvider.updateHub,
          resolveLatestHub: (homeId, hubId) {
            if (homeProvider.currentHome?.id != homeId) return null;
            for (final hub in homeProvider.currentHomeHubs) {
              if (hub.id == hubId) return hub;
            }
            return null;
          },
          onEnabled: (_) => syncProvider.connectIfAvailable(),
        );
      }
      _showSnackBar(
        enabled
            ? routeVerified
                ? 'Remote access enabled.'
                : 'Remote access is starting in the background.'
            : 'Remote access disabled.',
      );
    } catch (error, stackTrace) {
      debugPrint('Remote access toggle failed: $error');
      debugPrint('$stackTrace');
      _showSnackBar(
        enabled
            ? 'Could not enable remote access.'
            : 'Could not disable remote access.',
        backgroundColor: Colors.red.shade400,
      );
    } finally {
      if (mounted) {
        setState(() => _isTogglingRemoteAccess = false);
      }
    }
  }

  void _showSnackBar(String message, {Color? backgroundColor}) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(message),
        behavior: SnackBarBehavior.floating,
        backgroundColor: backgroundColor,
      ),
    );
  }

  Widget _buildSwitchRow({
    required IconData icon,
    required Color iconColor,
    required String label,
    required String statusText,
    required Color statusColor,
    required bool value,
    required Color activeTrackColor,
    required ValueChanged<bool>? onChanged,
    String? tooltip,
    bool busy = false,
  }) {
    return Container(
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(
          color: CelestialColors.orbitRing.withValues(alpha: 0.5),
        ),
      ),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
        child: Row(
          children: [
            Container(
              width: 40,
              height: 40,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: iconColor.withValues(alpha: 0.16),
              ),
              child: Icon(
                icon,
                color: iconColor,
                size: 20,
              ),
            ),
            const SizedBox(width: 14),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      Flexible(
                        child: Text(
                          label,
                          overflow: TextOverflow.ellipsis,
                          style: const TextStyle(
                            color: CelestialColors.textPrimary,
                            fontSize: 15,
                            fontWeight: FontWeight.w600,
                          ),
                        ),
                      ),
                      if (tooltip != null) ...[
                        const SizedBox(width: 4),
                        InfoTooltip(
                          message: tooltip,
                          iconSize: 13,
                          accentColor: statusColor,
                        ),
                      ],
                    ],
                  ),
                  const SizedBox(height: 4),
                  Text(
                    statusText,
                    style: TextStyle(
                      color: statusColor,
                      fontSize: 13,
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                ],
              ),
            ),
            const SizedBox(width: 12),
            if (busy)
              SizedBox(
                width: 22,
                height: 22,
                child: CircularProgressIndicator(
                  strokeWidth: 2,
                  valueColor: AlwaysStoppedAnimation(statusColor),
                ),
              )
            else
              Switch.adaptive(
                value: value,
                activeTrackColor: activeTrackColor,
                onChanged: onChanged,
              ),
          ],
        ),
      ),
    );
  }

  Widget _buildSection({
    required String title,
    required List<Widget> children,
  }) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.only(left: 4, bottom: 10),
          child: Text(
            title,
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.6),
              fontSize: 13,
              fontWeight: FontWeight.w500,
              letterSpacing: 0.8,
            ),
          ),
        ),
        ...children,
      ],
    );
  }
}

class _DeviceCounts {
  int lights = 0;
  int buttons = 0;
  int motion = 0;
}

class RhythmServerHubManagementSection extends StatefulWidget {
  const RhythmServerHubManagementSection({
    super.key,
    this.showConfigured = true,
    this.showAddOptions = true,
    this.showMatterAddOption = true,
    this.onResynced,
  });

  /// Render the list of already-paired hubs (and their devices).
  final bool showConfigured;

  /// Render the "add a hub / pair a device" options. Splitting these two lets
  /// the same capability-aware section back both the Settings → Devices list
  /// (configured only) and the "+" Add Device flow (add options only).
  final bool showAddOptions;

  /// Render the direct Matter pairing row. Add & Review owns its universal
  /// Add Bulb row, while Devices settings keeps the existing direct option.
  final bool showMatterAddOption;

  /// Called after a "Re-Sync" completes, so a host (e.g. the Add & Review
  /// screen) can reload anything derived from the fresh device data.
  final VoidCallback? onResynced;

  @override
  State<RhythmServerHubManagementSection> createState() =>
      _RhythmServerHubManagementSectionState();
}

class _RhythmServerHubManagementSectionState
    extends State<RhythmServerHubManagementSection> {
  bool _isFetchingSummaries = false;
  bool _hubSummariesLoaded = false;
  bool _isResyncing = false;
  Map<String, String> _hubSummaries = {};

  @override
  void initState() {
    super.initState();
    unawaited(_fetchHubSummaries());
  }

  @override
  Widget build(BuildContext context) {
    final syncProvider = context.watch<ServerSyncProvider>();
    final configuredHubs = syncProvider.serverHubInfos
        .where((h) => h['type'] != null && h['type'] != 'none')
        .toList();
    final shouldRefreshSummaries =
        syncProvider.connectionState == RhythmConnectionState.connected &&
            configuredHubs.any((hub) {
              final type = hub['type'] as String?;
              return type != null && !_hubSummaries.containsKey(type);
            });

    if (shouldRefreshSummaries && !_isFetchingSummaries) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) {
          unawaited(_fetchHubSummaries());
        }
      });
    }

    final sections = <Widget>[];
    if (widget.showConfigured) {
      final configuredSection = _buildConfiguredHubsSection(configuredHubs);
      if (configuredSection != null) {
        sections.add(configuredSection);
      }
    }

    if (widget.showAddOptions) {
      final addHubSection =
          _buildHubPairingSuggestions(syncProvider, configuredHubs);
      if (addHubSection != null) {
        if (sections.isNotEmpty) {
          sections.add(const SizedBox(height: 12));
        }
        sections.add(addHubSection);
      }
    }

    if (sections.isEmpty) return const SizedBox.shrink();

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: sections,
    );
  }

  Future<void> _fetchHubSummaries() async {
    if (_isFetchingSummaries) return;
    _isFetchingSummaries = true;
    try {
      final http = context.read<RhythmConnection>();
      final devices = await http.api.getCanonicalDevices();
      if (!mounted) return;

      if (devices == null) {
        setState(() => _hubSummariesLoaded = true);
        return;
      }

      final counts = <String, _DeviceCounts>{};
      for (final d in devices) {
        final dtype = d['device_type'] as String? ?? 'light';
        final endpoints = d['endpoints'] as List<dynamic>? ?? [];
        final hubTypes = <String>{};
        for (final ep in endpoints) {
          final hubKey = (ep as Map<String, dynamic>)['hub_key']
                  as Map<String, dynamic>? ??
              {};
          final ht = hubKey['hub_type']?.toString();
          if (ht != null) hubTypes.add(ht);
        }
        for (final ht in hubTypes) {
          final countsForHub = counts[ht] ??= _DeviceCounts();
          if (dtype == 'light') {
            countsForHub.lights++;
          } else if (dtype == 'button') {
            countsForHub.buttons++;
          } else if (dtype == 'motion') {
            countsForHub.motion++;
          }
        }
      }

      final summaries = <String, String>{};
      for (final entry in counts.entries) {
        final countsForHub = entry.value;
        final parts = <String>[];
        if (countsForHub.lights > 0) {
          parts.add(
              '${countsForHub.lights} light${countsForHub.lights > 1 ? 's' : ''}');
        }
        if (countsForHub.buttons > 0) {
          parts.add(
              '${countsForHub.buttons} button${countsForHub.buttons > 1 ? 's' : ''}');
        }
        if (countsForHub.motion > 0) {
          parts.add(
              '${countsForHub.motion} sensor${countsForHub.motion > 1 ? 's' : ''}');
        }
        summaries[entry.key] = parts.isEmpty ? 'No devices' : parts.join(', ');
      }

      setState(() {
        _hubSummaries = summaries;
        _hubSummariesLoaded = true;
      });
    } finally {
      _isFetchingSummaries = false;
    }
  }

  Widget? _buildConfiguredHubsSection(
      List<Map<String, dynamic>> configuredHubs) {
    if (configuredHubs.isEmpty) return null;

    return _buildSection(
      title: configuredHubs.length > 1 ? 'LIGHT HUBS' : 'LIGHT HUB',
      children: [
        Container(
          decoration: BoxDecoration(
            color: CelestialColors.backgroundCard,
            borderRadius: BorderRadius.circular(14),
            border: Border.all(
              color: CelestialColors.orbitRing.withValues(alpha: 0.5),
            ),
          ),
          child: Column(
            children: [
              for (final (index, hub) in configuredHubs.indexed) ...[
                if (index > 0)
                  Divider(
                    height: 1,
                    color: CelestialColors.orbitRing.withValues(alpha: 0.3),
                  ),
                _buildHubRow(hub),
              ],
            ],
          ),
        ),
      ],
    );
  }

  Widget? _buildHubPairingSuggestions(
    ServerSyncProvider syncProvider,
    List<Map<String, dynamic>> configuredHubs,
  ) {
    final matterOptions = widget.showMatterAddOption
        ? _buildMatterAddOptionRows(syncProvider)
        : const <Widget>[];

    Widget divider() => Divider(
          height: 1,
          color: CelestialColors.orbitRing.withValues(alpha: 0.3),
        );

    Container card(List<Widget> children) => Container(
          decoration: BoxDecoration(
            color: CelestialColors.backgroundCard,
            borderRadius: BorderRadius.circular(14),
            border: Border.all(
              color: CelestialColors.orbitRing.withValues(alpha: 0.5),
            ),
          ),
          child: Column(children: children),
        );

    // Home Assistant and Hue can't be paired from the app — they're managed on
    // the hub itself. Surface them as read-only links into the Devices list,
    // and rely on Re-Sync to pull in whatever bulbs have been paired there.
    // Matter *can* be added directly, so it sits in its own card below the
    // read-only group (Re-Sync doesn't apply to it).
    return _buildSection(
      title: 'Add Device',
      children: [
        card([
          _buildHubOptionRow(
            icon: Icons.home_outlined,
            label: 'Home Assistant',
            color: const Color(0xFF42A5F5),
            showBetaBadge: true,
            trailingLabel: 'Devices',
            onTap: () => DevicesListScreen.show(context),
          ),
          divider(),
          _buildHubOptionRow(
            icon: Icons.lightbulb_outline,
            label: 'Philips Hue',
            color: const Color(0xFFFFB900),
            trailingLabel: 'Devices',
            onTap: () => DevicesListScreen.show(context),
          ),
          divider(),
          _buildResyncRow(),
        ]),
        if (matterOptions.isNotEmpty) ...[
          const SizedBox(height: 12),
          card(matterOptions),
        ],
      ],
    );
  }

  /// Prominent "Re-Sync" action grouped with the read-only Home Assistant / Hue
  /// links — pulls in whatever bulbs have been paired to those hubs.
  Widget _buildResyncRow() {
    const accent = CelestialColors.accentBlue;
    return GestureDetector(
      behavior: HitTestBehavior.opaque,
      onTap: _isResyncing ? null : _resync,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
        child: Row(
          children: [
            if (_isResyncing)
              const SizedBox(
                width: 20,
                height: 20,
                child: CircularProgressIndicator(
                  strokeWidth: 2,
                  valueColor: AlwaysStoppedAnimation(accent),
                ),
              )
            else
              const Icon(Icons.sync_rounded, color: accent, size: 20),
            const SizedBox(width: 12),
            Expanded(
              child: Text(
                _isResyncing ? 'Re-Syncing…' : 'Re-Sync',
                style: const TextStyle(
                  color: accent,
                  fontSize: 14,
                  fontWeight: FontWeight.w700,
                ),
              ),
            ),
            Text(
              'Add paired bulbs',
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.5),
                fontSize: 12,
              ),
            ),
          ],
        ),
      ),
    );
  }

  Future<void> _resync() async {
    if (_isResyncing) return;
    setState(() => _isResyncing = true);
    final syncProvider = context.read<ServerSyncProvider>();
    try {
      await syncProvider.api.triggerSync();
      await syncProvider.fullRefresh();
      await _fetchHubSummaries();
      widget.onResynced?.call();
    } catch (e, st) {
      debugPrint('RhythmServerHubManagementSection: resync failed: $e\n$st');
    } finally {
      if (mounted) setState(() => _isResyncing = false);
    }
  }

  List<Widget> _buildMatterAddOptionRows(ServerSyncProvider syncProvider) {
    if (!syncProvider.canAddMatterDevice) return const [];
    return [
      _buildHubOptionRow(
        icon: Icons.memory_outlined,
        label: 'Add Matter Device',
        color: const Color(0xFF26A69A),
        showBetaBadge: true,
        onTap: () => _startMatterAddFlow(),
      ),
    ];
  }

  Future<void> _startMatterAddFlow() async {
    await startMatterPairingFlow(context);
    if (!mounted) return;
    await _fetchHubSummaries();
  }

  Widget _buildHubRow(Map<String, dynamic> hubInfo) {
    final type = hubInfo['type'] as String;
    final label = _RhythmServerSettingsScreenState._hubLabel(type);
    final hubColor = _RhythmServerSettingsScreenState._hubColor(type);
    final connected = hubInfo['connected'] as bool? ?? false;
    final statusColor =
        _RhythmServerSettingsScreenState._hubConnectionColor(hubInfo);
    final deviceSummary = _hubSummaries[type] ??
        (_hubSummariesLoaded
            ? (connected ? 'Connected · no devices' : 'No devices')
            : 'Loading...');
    final subtitle = _RhythmServerSettingsScreenState._hubRowSubtitle(
        hubInfo, deviceSummary);

    return GestureDetector(
      onTap: () => _HubDetailScreen.show(context, hubInfo: hubInfo),
      behavior: HitTestBehavior.opaque,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
        child: Row(
          children: [
            Icon(
              _RhythmServerSettingsScreenState._hubIcon(type),
              color: hubColor.withValues(alpha: connected ? 1.0 : 0.5),
              size: 20,
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  if (_RhythmServerSettingsScreenState._hubHasBetaBadge(type))
                    BetaLabel(
                      label: label,
                      style: const TextStyle(
                        color: CelestialColors.textPrimary,
                        fontSize: 14,
                        fontWeight: FontWeight.w500,
                      ),
                    )
                  else
                    Text(
                      label,
                      style: const TextStyle(
                        color: CelestialColors.textPrimary,
                        fontSize: 14,
                        fontWeight: FontWeight.w500,
                      ),
                    ),
                  const SizedBox(height: 2),
                  Text(
                    subtitle,
                    style: TextStyle(
                      color:
                          CelestialColors.textSecondary.withValues(alpha: 0.6),
                      fontSize: 12,
                    ),
                  ),
                ],
              ),
            ),
            Container(
              width: 8,
              height: 8,
              margin: const EdgeInsets.only(right: 8),
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: statusColor,
                boxShadow: !connected
                    ? [
                        BoxShadow(
                          color: statusColor.withValues(alpha: 0.6),
                          blurRadius: 6,
                          spreadRadius: 1,
                        ),
                      ]
                    : null,
              ),
            ),
            Icon(
              Icons.chevron_right,
              color: CelestialColors.textSecondary.withValues(alpha: 0.4),
              size: 20,
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildHubOptionRow({
    required IconData icon,
    required String label,
    required Color color,
    bool isActive = false,
    bool isLoading = false,
    bool showBetaBadge = false,
    String? trailingLabel,
    VoidCallback? onTap,
  }) {
    return GestureDetector(
      onTap: onTap,
      behavior: HitTestBehavior.opaque,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
        child: Row(
          children: [
            Icon(
              icon,
              color: isActive ? color : color.withValues(alpha: 0.6),
              size: 20,
            ),
            const SizedBox(width: 12),
            Expanded(
              child: showBetaBadge
                  ? BetaLabel(
                      label: label,
                      style: const TextStyle(
                        color: CelestialColors.textPrimary,
                        fontSize: 14,
                        fontWeight: FontWeight.w500,
                      ),
                    )
                  : Text(
                      label,
                      style: const TextStyle(
                        color: CelestialColors.textPrimary,
                        fontSize: 14,
                        fontWeight: FontWeight.w500,
                      ),
                    ),
            ),
            if (isLoading)
              SizedBox(
                width: 16,
                height: 16,
                child: CircularProgressIndicator(
                  strokeWidth: 2,
                  valueColor: AlwaysStoppedAnimation(color),
                ),
              )
            else if (isActive)
              Container(
                padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
                decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(6),
                  color: const Color(0xFF22C55E).withValues(alpha: 0.15),
                ),
                child: const Text(
                  'Active',
                  style: TextStyle(
                    color: Color(0xFF22C55E),
                    fontSize: 11,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              )
            else if (onTap != null) ...[
              if (trailingLabel != null) ...[
                Text(
                  trailingLabel,
                  style: TextStyle(
                    color: CelestialColors.textSecondary.withValues(alpha: 0.4),
                    fontSize: 13,
                  ),
                ),
                const SizedBox(width: 4),
              ],
              Icon(
                Icons.chevron_right,
                color: CelestialColors.textSecondary.withValues(alpha: 0.4),
                size: 20,
              ),
            ],
          ],
        ),
      ),
    );
  }

  Widget _buildSection({
    required String title,
    required List<Widget> children,
  }) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.only(left: 4, bottom: 10),
          child: Text(
            title,
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.6),
              fontSize: 13,
              fontWeight: FontWeight.w500,
              letterSpacing: 0.8,
            ),
          ),
        ),
        ...children,
      ],
    );
  }
}

// =============================================================================
// Reboot Overlay — polls health while the device cycles, dismisses when back
// =============================================================================

class _RebootOverlay extends StatefulWidget {
  final RhythmDiagnosticsApi client;
  final String headerTitle;

  const _RebootOverlay({required this.client, required this.headerTitle});

  static Future<bool> show(
    BuildContext context, {
    required RhythmDiagnosticsApi client,
    required String headerTitle,
  }) async {
    final result = await Navigator.of(context, rootNavigator: true).push<bool>(
      PageRouteBuilder(
        opaque: true,
        pageBuilder: (_, __, ___) => _RebootOverlay(
          client: client,
          headerTitle: headerTitle,
        ),
        transitionsBuilder: (_, animation, __, child) {
          return FadeTransition(opacity: animation, child: child);
        },
        transitionDuration: const Duration(milliseconds: 300),
      ),
    );
    return result ?? false;
  }

  @override
  State<_RebootOverlay> createState() => _RebootOverlayState();
}

class _RebootOverlayState extends State<_RebootOverlay>
    with SingleTickerProviderStateMixin {
  // RPi Zero cold-boot fits comfortably in 90s. Self-update polls for 3min,
  // but a bare reboot is much faster than an OTA so this is tighter.
  static const _deadline = Duration(seconds: 90);
  static const _pollInterval = Duration(seconds: 3);
  static const _teal = Color(0xFF00BCD4);

  late final AnimationController _pulseController;
  late final Animation<double> _pulseAnimation;
  Timer? _pollTimer;
  Timer? _deadlineTimer;
  bool _sawDisconnect = false;
  bool _timedOut = false;
  bool _finished = false;

  @override
  void initState() {
    super.initState();
    _pulseController = AnimationController(
      duration: const Duration(milliseconds: 2000),
      vsync: this,
    )..repeat(reverse: true);
    _pulseAnimation = Tween<double>(begin: 0.3, end: 0.8).animate(
      CurvedAnimation(parent: _pulseController, curve: Curves.easeInOut),
    );

    _deadlineTimer = Timer(_deadline, _onTimeout);
    // Server sleeps 2s before invoking /sbin/reboot; delay first poll so we
    // don't get a stale "healthy" from the pre-reboot process.
    Future.delayed(const Duration(seconds: 3), _poll);
    _pollTimer = Timer.periodic(_pollInterval, (_) => _poll());
  }

  Future<void> _poll() async {
    if (_finished || !mounted) return;
    final ok = await widget.client.healthCheck();
    if (_finished || !mounted) return;
    if (!ok) {
      if (!_sawDisconnect) setState(() => _sawDisconnect = true);
      return;
    }
    if (_sawDisconnect) {
      _finish(success: true);
    }
  }

  void _onTimeout() {
    if (_finished) return;
    _pollTimer?.cancel();
    setState(() => _timedOut = true);
  }

  void _finish({required bool success}) {
    if (_finished) return;
    _finished = true;
    _pollTimer?.cancel();
    _deadlineTimer?.cancel();
    Navigator.of(context).pop(success);
  }

  @override
  void dispose() {
    _pollTimer?.cancel();
    _deadlineTimer?.cancel();
    _pulseController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return PopScope(
      canPop: false,
      onPopInvokedWithResult: (didPop, _) {
        if (!didPop && _timedOut) _finish(success: false);
      },
      child: Scaffold(
        backgroundColor: CelestialColors.backgroundDark,
        body: SafeArea(
          child: Center(
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 40),
              child: _timedOut ? _buildTimeout() : _buildWaiting(),
            ),
          ),
        ),
      ),
    );
  }

  Widget _buildWaiting() {
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        AnimatedBuilder(
          animation: _pulseAnimation,
          builder: (_, __) => Container(
            width: 92,
            height: 92,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              gradient: RadialGradient(
                colors: [
                  _teal.withValues(alpha: 0.18),
                  _teal.withValues(alpha: 0.04),
                ],
              ),
              boxShadow: [
                BoxShadow(
                  color: _teal.withValues(
                    alpha: _pulseAnimation.value * 0.28,
                  ),
                  blurRadius: 32,
                  spreadRadius: 4,
                ),
              ],
            ),
            child:
                const Icon(Icons.restart_alt_rounded, color: _teal, size: 38),
          ),
        ),
        const SizedBox(height: 28),
        Text(
          'Rebooting ${widget.headerTitle}',
          textAlign: TextAlign.center,
          style: const TextStyle(
            color: CelestialColors.textPrimary,
            fontSize: 22,
            fontWeight: FontWeight.w600,
            letterSpacing: 0.2,
          ),
        ),
        const SizedBox(height: 12),
        Text(
          _sawDisconnect
              ? 'Waiting for ${widget.headerTitle} to come back online...'
              : 'Powering down...',
          textAlign: TextAlign.center,
          style: TextStyle(
            color: _teal.withValues(alpha: 0.75),
            fontSize: 14,
          ),
        ),
        const SizedBox(height: 28),
        Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Icon(
              Icons.info_outline_rounded,
              color: CelestialColors.textSecondary.withValues(alpha: 0.4),
              size: 16,
            ),
            const SizedBox(width: 8),
            Text(
              'Do not close the app',
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.4),
                fontSize: 13,
              ),
            ),
          ],
        ),
      ],
    );
  }

  Widget _buildTimeout() {
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        Container(
          width: 80,
          height: 80,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: Colors.red.shade400.withValues(alpha: 0.15),
          ),
          child: Icon(
            Icons.error_outline_rounded,
            color: Colors.red.shade400,
            size: 40,
          ),
        ),
        const SizedBox(height: 32),
        const Text(
          'Reboot Timed Out',
          style: TextStyle(
            color: CelestialColors.textPrimary,
            fontSize: 22,
            fontWeight: FontWeight.w600,
          ),
        ),
        const SizedBox(height: 8),
        Text(
          '${widget.headerTitle} did not come back online within '
          '${_deadline.inSeconds}s.',
          textAlign: TextAlign.center,
          style: TextStyle(
            color: Colors.red.shade400,
            fontSize: 14,
          ),
        ),
        const SizedBox(height: 40),
        GestureDetector(
          onTap: () => _finish(success: false),
          child: Container(
            width: double.infinity,
            padding: const EdgeInsets.symmetric(vertical: 14),
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(14),
              color: CelestialColors.textSecondary.withValues(alpha: 0.08),
              border: Border.all(
                color: CelestialColors.textSecondary.withValues(alpha: 0.2),
              ),
            ),
            child: const Text(
              'Close',
              textAlign: TextAlign.center,
              style: TextStyle(
                color: CelestialColors.textSecondary,
                fontSize: 16,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
        ),
      ],
    );
  }
}

// =============================================================================
// Diagnostics Screen (full-screen slide-up with Overview + Logs tabs)
// =============================================================================

class _RhythmServerDiagnosticsScreen extends StatefulWidget {
  final RhythmDiagnosticsApi client;

  const _RhythmServerDiagnosticsScreen({required this.client});

  @override
  State<_RhythmServerDiagnosticsScreen> createState() =>
      _RhythmServerDiagnosticsScreenState();
}

class _RhythmServerDiagnosticsScreenState
    extends State<_RhythmServerDiagnosticsScreen> {
  // Logs
  List<Map<String, dynamic>>? _logs;
  bool _loadingLogs = true;
  String? _categoryFilter;

  static const _teal = Color(0xFF00BCD4);
  static const _logCategories = ['all', 'conn', 'hub', 'cmd', 'evt', 'sys'];

  @override
  void initState() {
    super.initState();
    _fetchLogs();
  }

  Future<void> _fetchLogs() async {
    setState(() => _loadingLogs = true);
    final logs = await widget.client.getDiagLogs(
      limit: 50,
      category: _categoryFilter,
    );
    if (mounted) {
      setState(() {
        _logs = logs;
        _loadingLogs = false;
      });
    }
  }

  Future<void> _handleRefresh() async {
    await _fetchLogs();
  }

  // ─── Log actions ─────────────────────────────────────────

  void _copyAllLogs() {
    if (_logs == null || _logs!.isEmpty) return;

    final buffer = StringBuffer();
    for (final log in _logs!) {
      final level = (log['level'] as String? ?? 'info').toUpperCase();
      final cat = log['cat'] as String? ?? 'sys';
      final msg = log['msg'] as String? ?? '';
      buffer.writeln('[$level] $cat: $msg');
    }

    Clipboard.setData(ClipboardData(text: buffer.toString()));

    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text('Copied ${_logs!.length} log entries'),
        backgroundColor: _teal,
        duration: const Duration(seconds: 2),
        behavior: SnackBarBehavior.floating,
        margin: const EdgeInsets.fromLTRB(16, 0, 16, 16),
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(10),
        ),
      ),
    );
  }

  // ─── Log helpers ─────────────────────────────────────────

  Color _levelColor(String level) {
    switch (level) {
      case 'error':
        return Colors.red.shade400;
      case 'warn':
        return Colors.amber;
      default:
        return CelestialColors.textSecondary;
    }
  }

  String _levelLabel(String level) {
    switch (level) {
      case 'error':
        return 'ERR';
      case 'warn':
        return 'WRN';
      default:
        return 'INF';
    }
  }

  Color _catColor(String cat) {
    switch (cat) {
      case 'conn':
        return _teal;
      case 'hub':
        return Colors.amber;
      case 'cmd':
        return const Color(0xFF22C55E);
      case 'evt':
        return Colors.purple.shade300;
      default:
        return CelestialColors.textSecondary;
    }
  }

  // ─── Build ────────────────────────────────────────────────

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            _buildHeader(_loadingLogs),
            Container(
              height: 1,
              color: CelestialColors.orbitRing.withValues(alpha: 0.2),
            ),
            Expanded(
              child: _buildLogsTab(),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildHeader(bool isLoading) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
      child: Row(
        children: [
          GestureDetector(
            onTap: () => Navigator.of(context).pop(),
            child: Container(
              width: 40,
              height: 40,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: _teal.withValues(alpha: 0.15),
                border: Border.all(
                  color: _teal.withValues(alpha: 0.3),
                  width: 1,
                ),
              ),
              child: const Icon(
                Icons.arrow_back_ios_new_rounded,
                color: _teal,
                size: 16,
              ),
            ),
          ),
          const Expanded(
            child: Text(
              'Diagnostics',
              textAlign: TextAlign.center,
              style: TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 18,
                fontWeight: FontWeight.w600,
                letterSpacing: 0.3,
              ),
            ),
          ),
          GestureDetector(
            onTap: isLoading ? null : _handleRefresh,
            child: Container(
              width: 40,
              height: 40,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: _teal.withValues(alpha: 0.15),
                border: Border.all(
                  color: _teal.withValues(alpha: 0.3),
                  width: 1,
                ),
              ),
              child: isLoading
                  ? const Padding(
                      padding: EdgeInsets.all(11),
                      child: CircularProgressIndicator(
                        strokeWidth: 2,
                        valueColor: AlwaysStoppedAnimation(_teal),
                      ),
                    )
                  : Icon(
                      Icons.refresh_rounded,
                      color: _teal.withValues(alpha: 0.8),
                      size: 18,
                    ),
            ),
          ),
        ],
      ),
    );
  }

  // ─── Logs tab ──────────────────────────────────────────────

  Widget _buildLogsTab() {
    return Column(
      children: [
        // Category filter chips + copy button
        Padding(
          padding: const EdgeInsets.only(top: 12, bottom: 12),
          child: SizedBox(
            height: 32,
            child: Row(
              children: [
                Expanded(
                  child: ListView(
                    scrollDirection: Axis.horizontal,
                    padding: const EdgeInsets.only(left: 16),
                    children: _logCategories.map((cat) {
                      final isSelected = cat == 'all'
                          ? _categoryFilter == null
                          : _categoryFilter == cat;
                      return Padding(
                        padding: const EdgeInsets.only(right: 6),
                        child: GestureDetector(
                          onTap: () {
                            setState(() {
                              _categoryFilter = cat == 'all' ? null : cat;
                            });
                            _fetchLogs();
                          },
                          child: Container(
                            padding: const EdgeInsets.symmetric(
                                horizontal: 12, vertical: 6),
                            decoration: BoxDecoration(
                              borderRadius: BorderRadius.circular(8),
                              color: isSelected
                                  ? _teal.withValues(alpha: 0.2)
                                  : Colors.white.withValues(alpha: 0.04),
                              border: Border.all(
                                color: isSelected
                                    ? _teal.withValues(alpha: 0.4)
                                    : CelestialColors.textSecondary
                                        .withValues(alpha: 0.15),
                              ),
                            ),
                            child: Text(
                              cat.toUpperCase(),
                              style: TextStyle(
                                color: isSelected
                                    ? _teal
                                    : CelestialColors.textSecondary,
                                fontSize: 11,
                                fontWeight: FontWeight.w600,
                                letterSpacing: 0.5,
                              ),
                            ),
                          ),
                        ),
                      );
                    }).toList(),
                  ),
                ),
                if (_logs != null && _logs!.isNotEmpty)
                  Padding(
                    padding: const EdgeInsets.only(right: 16, left: 4),
                    child: GestureDetector(
                      onTap: _copyAllLogs,
                      child: Container(
                        padding: const EdgeInsets.symmetric(
                            horizontal: 10, vertical: 6),
                        decoration: BoxDecoration(
                          borderRadius: BorderRadius.circular(8),
                          color: Colors.white.withValues(alpha: 0.04),
                          border: Border.all(
                            color: CelestialColors.textSecondary
                                .withValues(alpha: 0.15),
                          ),
                        ),
                        child: Row(
                          mainAxisSize: MainAxisSize.min,
                          children: [
                            Icon(
                              Icons.copy_rounded,
                              color: CelestialColors.textSecondary,
                              size: 13,
                            ),
                            const SizedBox(width: 4),
                            Text(
                              'Copy',
                              style: TextStyle(
                                color: CelestialColors.textSecondary,
                                fontSize: 11,
                                fontWeight: FontWeight.w600,
                                letterSpacing: 0.5,
                              ),
                            ),
                          ],
                        ),
                      ),
                    ),
                  ),
              ],
            ),
          ),
        ),
        Container(
          height: 1,
          color: CelestialColors.orbitRing.withValues(alpha: 0.2),
        ),
        Expanded(
          child: _loadingLogs
              ? const Center(
                  child: SizedBox(
                    width: 20,
                    height: 20,
                    child: CircularProgressIndicator(
                      strokeWidth: 2,
                      valueColor: AlwaysStoppedAnimation(_teal),
                    ),
                  ),
                )
              : _logs == null || _logs!.isEmpty
                  ? Center(
                      child: Text(
                        _logs == null
                            ? 'Failed to load logs'
                            : 'No log entries',
                        style: TextStyle(
                          color: CelestialColors.textSecondary
                              .withValues(alpha: 0.6),
                          fontSize: 14,
                        ),
                      ),
                    )
                  : ListView.builder(
                      padding: const EdgeInsets.fromLTRB(12, 8, 12, 24),
                      itemCount: _logs!.length,
                      itemBuilder: (context, index) =>
                          _buildLogEntry(_logs![index]),
                    ),
        ),
      ],
    );
  }

  Widget _buildLogEntry(Map<String, dynamic> log) {
    final level = log['level'] as String? ?? 'info';
    final cat = log['cat'] as String? ?? 'sys';
    final msg = log['msg'] as String? ?? '';

    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 3),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // Level badge
          SizedBox(
            width: 30,
            child: Text(
              _levelLabel(level),
              style: TextStyle(
                color: _levelColor(level),
                fontSize: 10,
                fontWeight: FontWeight.w700,
                fontFamily: 'monospace',
              ),
            ),
          ),
          // Category badge
          Container(
            width: 36,
            alignment: Alignment.center,
            padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 1),
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(3),
              color: _catColor(cat).withValues(alpha: 0.15),
            ),
            child: Text(
              cat,
              style: TextStyle(
                color: _catColor(cat),
                fontSize: 9,
                fontWeight: FontWeight.w600,
                fontFamily: 'monospace',
              ),
            ),
          ),
          const SizedBox(width: 8),
          // Message
          Expanded(
            child: Text(
              msg,
              style: TextStyle(
                color: level == 'error'
                    ? Colors.red.shade300
                    : CelestialColors.textPrimary.withValues(alpha: 0.85),
                fontSize: 12,
                fontFamily: 'monospace',
                height: 1.4,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

// =============================================================================
// Hub Detail Screen — drill-down for a single light hub
// =============================================================================

class _HubDetailScreen extends StatefulWidget {
  final Map<String, dynamic> hubInfo;

  const _HubDetailScreen({required this.hubInfo});

  static Future<void> show(BuildContext context,
      {required Map<String, dynamic> hubInfo}) {
    return Navigator.of(context).push(
      PageRouteBuilder(
        opaque: false,
        barrierColor: Colors.black54,
        pageBuilder: (context, animation, secondaryAnimation) {
          return _HubDetailScreen(hubInfo: hubInfo);
        },
        transitionsBuilder: (context, animation, secondaryAnimation, child) {
          final curve = CurvedAnimation(
            parent: animation,
            curve: Curves.easeOutCubic,
            reverseCurve: Curves.easeInCubic,
          );
          return SlideTransition(
            position: Tween<Offset>(
              begin: const Offset(1, 0),
              end: Offset.zero,
            ).animate(curve),
            child: child,
          );
        },
        transitionDuration: const Duration(milliseconds: 300),
        reverseTransitionDuration: const Duration(milliseconds: 250),
      ),
    );
  }

  @override
  State<_HubDetailScreen> createState() => _HubDetailScreenState();
}

class _HubDetailScreenState extends State<_HubDetailScreen> {
  static const _teal = Color(0xFF00BCD4);

  String get _type => widget.hubInfo['type'] as String;
  String? get _address => widget.hubInfo['address'] as String?;

  // Canonical devices grouped into rooms for this hub type.
  List<RhythmRoom>? _canonicalRooms;
  String? _canonicalSummary;
  bool _canonicalLoading = false;

  @override
  void initState() {
    super.initState();
    _fetchCanonicalDevices();
  }

  Future<void> _fetchCanonicalDevices() async {
    setState(() => _canonicalLoading = true);
    final http = context.read<RhythmConnection>();
    final devices = await http.api.getCanonicalDevices();
    if (!mounted) return;

    if (devices == null) {
      setState(() {
        _canonicalRooms = [];
        _canonicalSummary = 'No devices';
        _canonicalLoading = false;
      });
      return;
    }

    // Filter to devices that have an endpoint matching this hub type.
    final hubDevices = devices.where((d) {
      final endpoints = d['endpoints'] as List<dynamic>? ?? [];
      return endpoints.any((ep) {
        final hubKey =
            (ep as Map<String, dynamic>)['hub_key'] as Map<String, dynamic>? ??
                {};
        return hubKey['hub_type']?.toString() == _type;
      });
    }).toList();

    // Build room-node-id -> room-name lookup from topology-derived room summaries.
    final syncProvider = context.read<ServerSyncProvider>();
    final roomNames = <String, String>{};
    for (final r in syncProvider.helloRooms) {
      if (r.id.isNotEmpty) roomNames[r.id] = r.name;
    }

    final parentNodeIdByDeviceId = <String, String?>{
      for (final node
          in syncProvider.topologyNodes.where((node) => node.isDevice))
        node.id: node.parentId,
    };

    // Group by parent node id -> build room/device summaries without relying
    // on legacy canonical `room_id` fields.
    final byRoom = <String?, List<Map<String, dynamic>>>{};
    for (final d in hubDevices) {
      final deviceId = d['id'] as String?;
      final parentNodeId = deviceId == null || deviceId.isEmpty
          ? d['parent_id'] as String? ?? d['room_id'] as String?
          : parentNodeIdByDeviceId[deviceId] ??
              d['parent_id'] as String? ??
              d['room_id'] as String?;
      (byRoom[parentNodeId] ??= []).add(d);
    }

    final parsedRooms = <RhythmRoom>[];
    for (final entry in byRoom.entries) {
      final roomDevices = entry.value
          .map((d) => RhythmDevice(
                id: d['id'] as String? ?? '',
                type: RhythmDeviceType.fromString(
                    d['device_type'] as String? ?? 'light'),
                name: d['name'] as String?,
                manufacturer: d['manufacturer'] as String?,
                model: d['model'] as String?,
              ))
          .toList();
      final roomId = entry.key;
      parsedRooms.add(RhythmRoom(
        id: roomId ?? '',
        name: (roomId != null ? roomNames[roomId] : null) ?? 'Unassigned',
        groupedLightId: '',
        state: RoomModeState.active,
        rhythmEnabled: false,
        disabled: false,
        timeOffset: 0,
        brightnessOffset: 0,
        hubTypes: [_type],
        devices: roomDevices,
      ));
    }

    // Summary
    final lights = hubDevices
        .where((d) => (d['device_type'] as String? ?? 'light') == 'light')
        .length;
    final buttons =
        hubDevices.where((d) => d['device_type'] == 'button').length;
    final motion = hubDevices.where((d) => d['device_type'] == 'motion').length;
    final parts = <String>[];
    if (lights > 0) parts.add('$lights light${lights > 1 ? 's' : ''}');
    if (buttons > 0) parts.add('$buttons button${buttons > 1 ? 's' : ''}');
    if (motion > 0) parts.add('$motion sensor${motion > 1 ? 's' : ''}');
    final roomCount = parsedRooms.length;
    final summary = parts.isEmpty
        ? 'No devices'
        : '${parts.join(', ')} across $roomCount room${roomCount != 1 ? 's' : ''}';

    setState(() {
      _canonicalRooms = parsedRooms;
      _canonicalSummary = summary;
      _canonicalLoading = false;
    });
  }

  @override
  Widget build(BuildContext context) {
    final syncProvider = context.watch<ServerSyncProvider>();
    final hubInfo = _currentHubInfo(syncProvider);
    final connected = hubInfo['connected'] as bool? ?? false;
    final connectionLabel =
        _RhythmServerSettingsScreenState._hubConnectionLabel(hubInfo);
    final connectionColor =
        _RhythmServerSettingsScreenState._hubConnectionColor(hubInfo);
    final retrySummary =
        _RhythmServerSettingsScreenState._hubRetrySummary(hubInfo);
    final retryActionEnabled =
        !_RhythmServerSettingsScreenState._hubIsAutoRetrying(hubInfo);
    final retryActionLabel = connected
        ? 'Reconnect'
        : _RhythmServerSettingsScreenState._hubIsAutoRetrying(hubInfo)
            ? 'Retrying Automatically'
            : 'Retry';
    final retryActionColor =
        _RhythmServerSettingsScreenState._hubIsAutoRetrying(hubInfo)
            ? _RhythmServerSettingsScreenState._warningAmber
            : _teal;
    final rooms = _canonicalRooms ?? [];
    final deviceSummary =
        _canonicalLoading ? 'Loading...' : (_canonicalSummary ?? 'No devices');
    final label = _RhythmServerSettingsScreenState._hubLabel(_type);
    final hubColor = _RhythmServerSettingsScreenState._hubColor(_type);
    final hubIcon = _RhythmServerSettingsScreenState._hubIcon(_type);
    final canAddMatter = syncProvider.canAddMatterDevice;
    const matterActionLabel = 'Add Matter Device';

    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            // Header
            Container(
              padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
              child: Row(
                children: [
                  GestureDetector(
                    onTap: () => Navigator.of(context).pop(),
                    child: Container(
                      width: 40,
                      height: 40,
                      decoration: BoxDecoration(
                        shape: BoxShape.circle,
                        color: hubColor.withValues(alpha: 0.15),
                        border: Border.all(
                          color: hubColor.withValues(alpha: 0.3),
                          width: 1,
                        ),
                      ),
                      child: Icon(
                        Icons.arrow_back,
                        color: hubColor,
                        size: 20,
                      ),
                    ),
                  ),
                  Expanded(
                    child: Center(
                      child: _RhythmServerSettingsScreenState._hubHasBetaBadge(
                              _type)
                          ? BetaLabel(
                              label: label,
                              style: TextStyle(
                                color: CelestialColors.textPrimary,
                                fontSize: 18,
                                fontWeight: FontWeight.w600,
                                letterSpacing: 0.3,
                              ),
                            )
                          : Text(
                              label,
                              textAlign: TextAlign.center,
                              style: const TextStyle(
                                color: CelestialColors.textPrimary,
                                fontSize: 18,
                                fontWeight: FontWeight.w600,
                                letterSpacing: 0.3,
                              ),
                            ),
                    ),
                  ),
                  const SizedBox(width: 40),
                ],
              ),
            ),
            Expanded(
              child: SingleChildScrollView(
                padding: const EdgeInsets.symmetric(horizontal: 24),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    const SizedBox(height: 8),
                    // Hero
                    Container(
                      padding: const EdgeInsets.all(24),
                      decoration: BoxDecoration(
                        borderRadius: BorderRadius.circular(24),
                        gradient: RadialGradient(
                          center: Alignment.center,
                          radius: 1.2,
                          colors: [
                            hubColor.withValues(alpha: connected ? 0.08 : 0.03),
                            CelestialColors.backgroundCard,
                          ],
                        ),
                        border: Border.all(
                          color:
                              hubColor.withValues(alpha: connected ? 0.2 : 0.1),
                          width: 1,
                        ),
                      ),
                      child: Column(
                        children: [
                          Container(
                            width: 56,
                            height: 56,
                            decoration: BoxDecoration(
                              shape: BoxShape.circle,
                              color: hubColor.withValues(
                                  alpha: connected ? 0.2 : 0.1),
                            ),
                            child: Icon(
                              hubIcon,
                              color: hubColor.withValues(
                                  alpha: connected ? 1.0 : 0.5),
                              size: 28,
                            ),
                          ),
                          const SizedBox(height: 12),
                          Row(
                            mainAxisAlignment: MainAxisAlignment.center,
                            children: [
                              Container(
                                width: 8,
                                height: 8,
                                decoration: BoxDecoration(
                                  shape: BoxShape.circle,
                                  color: connectionColor,
                                ),
                              ),
                              const SizedBox(width: 8),
                              Text(
                                connectionLabel,
                                style: TextStyle(
                                  color: connectionColor,
                                  fontSize: 14,
                                  fontWeight: FontWeight.w500,
                                ),
                              ),
                            ],
                          ),
                          if (retrySummary != null) ...[
                            const SizedBox(height: 6),
                            Text(
                              retrySummary,
                              textAlign: TextAlign.center,
                              style: TextStyle(
                                color: connectionColor.withValues(alpha: 0.9),
                                fontSize: 12,
                                fontWeight: FontWeight.w500,
                              ),
                            ),
                          ],
                          if (_address != null && _address!.isNotEmpty) ...[
                            const SizedBox(height: 4),
                            Text(
                              _address!,
                              style: TextStyle(
                                color: CelestialColors.textSecondary
                                    .withValues(alpha: 0.7),
                                fontSize: 13,
                                fontFamily: 'monospace',
                              ),
                            ),
                          ],
                          const SizedBox(height: 4),
                          Text(
                            deviceSummary,
                            style: TextStyle(
                              color: CelestialColors.textSecondary
                                  .withValues(alpha: 0.6),
                              fontSize: 12,
                            ),
                          ),
                        ],
                      ),
                    ),
                    const SizedBox(height: 20),
                    // Devices section
                    _buildSectionHeader('DEVICES'),
                    const SizedBox(height: 8),
                    _buildDevicesCard(rooms),
                    if (_type == 'matter' && canAddMatter) ...[
                      const SizedBox(height: 10),
                      _buildActionButton(
                        icon: Icons.add_circle_outline,
                        label: matterActionLabel,
                        color: const Color(0xFF26A69A),
                        onTap: () async {
                          await startMatterPairingFlow(context);
                          if (!mounted) return;
                          await _fetchCanonicalDevices();
                        },
                      ),
                    ],
                    if (_type != 'matter') ...[
                      const SizedBox(height: 24),
                      // Actions
                      _buildActionButton(
                        icon: Icons.refresh_rounded,
                        label: retryActionLabel,
                        color: retryActionColor,
                        onTap: retryActionEnabled
                            ? () => _retryHub(hubInfo)
                            : null,
                      ),
                      const SizedBox(height: 10),
                      _buildActionButton(
                        icon: Icons.link_off_rounded,
                        label: 'Disconnect',
                        color: Colors.red.shade400,
                        onTap: () => _disconnectHub(),
                      ),
                    ],
                    const SizedBox(height: 40),
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  // ─── Devices Card ────────────────────────────────────────

  Widget _buildDevicesCard(List<RhythmRoom> rooms) {
    return Container(
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(
          color: CelestialColors.orbitRing.withValues(alpha: 0.5),
        ),
      ),
      child: rooms.isEmpty
          ? Padding(
              padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 20),
              child: Center(
                child: Text(
                  'No devices discovered',
                  style: TextStyle(
                    color: CelestialColors.textSecondary.withValues(alpha: 0.5),
                    fontSize: 13,
                  ),
                ),
              ),
            )
          : Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                for (final (index, room) in rooms.indexed) ...[
                  if (index > 0)
                    Divider(
                        height: 1,
                        color:
                            CelestialColors.orbitRing.withValues(alpha: 0.2)),
                  // Room header
                  Padding(
                    padding: const EdgeInsets.only(
                        left: 16, right: 16, top: 12, bottom: 4),
                    child: Row(
                      children: [
                        Icon(
                          Icons.meeting_room_outlined,
                          color: CelestialColors.textSecondary
                              .withValues(alpha: 0.4),
                          size: 14,
                        ),
                        const SizedBox(width: 6),
                        Text(
                          room.name,
                          style: TextStyle(
                            color: CelestialColors.textSecondary
                                .withValues(alpha: 0.6),
                            fontSize: 12,
                            fontWeight: FontWeight.w600,
                            letterSpacing: 0.3,
                          ),
                        ),
                        const Spacer(),
                        Text(
                          room.deviceSummary,
                          style: TextStyle(
                            color: CelestialColors.textSecondary
                                .withValues(alpha: 0.4),
                            fontSize: 11,
                          ),
                        ),
                      ],
                    ),
                  ),
                  // Devices
                  if (room.devices.isEmpty)
                    Padding(
                      padding: const EdgeInsets.only(left: 36, bottom: 10),
                      child: Text(
                        '${room.deviceIds.length} device${room.deviceIds.length != 1 ? 's' : ''} (untyped)',
                        style: TextStyle(
                          color: CelestialColors.textSecondary
                              .withValues(alpha: 0.4),
                          fontSize: 12,
                        ),
                      ),
                    )
                  else
                    Padding(
                      padding: const EdgeInsets.only(bottom: 8),
                      child: Column(
                        children: [
                          for (final device in _sortDevices(room.devices))
                            _buildDeviceRow(device, room.id),
                        ],
                      ),
                    ),
                ],
              ],
            ),
    );
  }

  Widget _buildDeviceRow(RhythmDevice device, String roomId) {
    final (icon, iconColor) = _iconForDeviceType(device.type);
    final typeLabel = switch (device.type) {
      RhythmDeviceType.light => 'Light',
      RhythmDeviceType.button => 'Button',
      RhythmDeviceType.motion => 'Motion',
      RhythmDeviceType.contact => 'Contact',
    };

    return GestureDetector(
      behavior: HitTestBehavior.opaque,
      onTap: () async {
        await DeviceDetailSheet.show(context, device, roomId);
        if (!mounted) return;
        await _fetchCanonicalDevices();
      },
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 6),
        child: Row(
          children: [
            Icon(icon, color: iconColor, size: 18),
            const SizedBox(width: 10),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    device.displayName,
                    style: const TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 13,
                    ),
                  ),
                  if (device.productInfo != null)
                    Text(
                      device.productInfo!,
                      style: TextStyle(
                        color: CelestialColors.textSecondary
                            .withValues(alpha: 0.5),
                        fontSize: 11,
                      ),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                    ),
                ],
              ),
            ),
            Text(
              typeLabel,
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.6),
                fontSize: 11,
              ),
            ),
          ],
        ),
      ),
    );
  }

  // ─── Helpers ─────────────────────────────────────────────

  Widget _buildSectionHeader(String title) {
    return Padding(
      padding: const EdgeInsets.only(left: 4),
      child: Text(
        title,
        style: TextStyle(
          color: CelestialColors.textSecondary.withValues(alpha: 0.6),
          fontSize: 13,
          fontWeight: FontWeight.w500,
          letterSpacing: 0.8,
        ),
      ),
    );
  }

  Widget _buildActionButton({
    required IconData icon,
    required String label,
    required Color color,
    VoidCallback? onTap,
  }) {
    final enabled = onTap != null;
    return GestureDetector(
      onTap: onTap,
      child: Container(
        padding: const EdgeInsets.symmetric(vertical: 14),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(14),
          color: color.withValues(alpha: enabled ? 0.1 : 0.05),
          border: Border.all(
            color: color.withValues(alpha: enabled ? 0.3 : 0.16),
          ),
        ),
        child: Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Icon(
              icon,
              color: color.withValues(alpha: enabled ? 1.0 : 0.55),
              size: 18,
            ),
            const SizedBox(width: 10),
            Text(
              label,
              style: TextStyle(
                color: color.withValues(alpha: enabled ? 1.0 : 0.55),
                fontSize: 15,
                fontWeight: FontWeight.w600,
              ),
            ),
          ],
        ),
      ),
    );
  }

  List<RhythmDevice> _sortDevices(List<RhythmDevice> devices) {
    return List<RhythmDevice>.from(devices)
      ..sort((a, b) {
        const order = {
          RhythmDeviceType.light: 0,
          RhythmDeviceType.button: 1,
          RhythmDeviceType.motion: 2,
          RhythmDeviceType.contact: 3,
        };
        return (order[a.type] ?? 3).compareTo(order[b.type] ?? 3);
      });
  }

  (IconData, Color) _iconForDeviceType(RhythmDeviceType type) => switch (type) {
        RhythmDeviceType.light => (
            Icons.lightbulb_outline,
            const Color(0xFFFFB74D)
          ),
        RhythmDeviceType.button => (
            Icons.touch_app_outlined,
            const Color(0xFF64B5F6)
          ),
        RhythmDeviceType.motion => (
            Icons.sensors_outlined,
            const Color(0xFF81C784)
          ),
        RhythmDeviceType.contact => (
            Icons.sensor_door_outlined,
            const Color(0xFFFFB74D)
          ),
      };

  Map<String, dynamic> _currentHubInfo(ServerSyncProvider syncProvider) {
    final address = _address;
    for (final hubInfo in syncProvider.serverHubInfos) {
      final type = hubInfo['type'] as String?;
      if (type != _type) continue;
      if (address == null || address.isEmpty) return hubInfo;
      if (hubInfo['address'] == address) return hubInfo;
    }
    return widget.hubInfo;
  }

  Future<void> _retryHub(Map<String, dynamic> hubInfo) async {
    final syncProvider = context.read<ServerSyncProvider>();
    final needsManualRetry =
        _RhythmServerSettingsScreenState._hubNeedsManualRetry(hubInfo);
    final address = hubInfo['address'] as String? ?? _address ?? '';
    if (needsManualRetry && address.isNotEmpty) {
      await syncProvider.retryHub(_type, address);
      return;
    }
    await syncProvider.connection.reconnect();
  }

  void _disconnectHub() async {
    final syncProvider = context.read<ServerSyncProvider>();
    if (_type == 'matter') {
      final confirmed = await showDialog<bool>(
        context: context,
        builder: (ctx) => AlertDialog(
          backgroundColor: CelestialColors.backgroundCard,
          title: const Text('Remove Matter Hub?',
              style: TextStyle(color: CelestialColors.textPrimary)),
          content: const Text(
            'This will remove all commissioned Matter devices. You will need to re-pair them to use them again.',
            style: TextStyle(color: CelestialColors.textSecondary),
          ),
          actions: [
            TextButton(
                onPressed: () => Navigator.pop(ctx, false),
                child: const Text('Cancel')),
            TextButton(
              onPressed: () => Navigator.pop(ctx, true),
              child:
                  Text('Remove', style: TextStyle(color: Colors.red.shade400)),
            ),
          ],
        ),
      );
      if (confirmed != true) return;
    }

    await syncProvider.disconnectOneHub(_type, _address ?? '');
    if (!mounted) return;
    syncProvider.connection.reconnect();
    Navigator.of(context).pop();
  }
}
