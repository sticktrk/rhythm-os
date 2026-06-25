import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show
        RhythmAuthApi,
        RhythmAuthStatus,
        RhythmConfigApi,
        RhythmConnection,
        RhythmConnectionState,
        RhythmDevice,
        RhythmDeviceType,
        RhythmDiagnosticsApi,
        RhythmHubInfo,
        RhythmHubStartupRetry,
        RhythmHubStartupRetryStatus,
        RhythmOtaUpdateProgress,
        RhythmOtaUpdateStage,
        RhythmRoom,
        RoomModeState;
import '../../config/feature_flags.dart';
import '../../models/plan_tier.dart';
import '../../widgets/solar_orbit.dart';
import '../../widgets/stage_timeline.dart';
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
import '../../widgets/plan_tier_modal.dart';
import '../../widgets/report_bug_flow.dart';
import 'ha_configurator_screen.dart';
import 'hue_configurator_screen.dart';
import 'matter_pairing_flow.dart';

String _formatOtaVersionLabel(String version) {
  return version.startsWith('v') || version.startsWith('V')
      ? version
      : 'v$version';
}

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
        builder: (_) => _RhythmServerAdvancedSettingsScreen(hub: _currentHub),
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
    _OtaUpdateOverlay.show(
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
  final Hub hub;

  const _RhythmServerAdvancedSettingsScreen({
    required this.hub,
  });

  @override
  State<_RhythmServerAdvancedSettingsScreen> createState() =>
      _RhythmServerAdvancedSettingsScreenState();
}

class _RhythmServerAdvancedSettingsScreenState
    extends State<_RhythmServerAdvancedSettingsScreen> {
  static const _teal = Color(0xFF00BCD4);
  static const _enabledGreen = Color(0xFF22C55E);
  static const _warningAmber = Color(0xFFE8A54B);
  static const _disabledRed = Color(0xFFEF4444);

  RhythmAuthStatus? _authStatus;
  String? _authToken;
  bool _isAuthLoading = true;
  bool _isRemoteAccessUpdating = false;

  bool get _hasAuthToken {
    final token = _authToken?.trim();
    return token != null && token.isNotEmpty;
  }

  Hub get _currentHub {
    final hubs = context.read<HomeProvider>().currentHomeHubs;
    return hubs
        .where((hub) => hub.id == widget.hub.id)
        .cast<Hub?>()
        .firstWhere((hub) => hub != null, orElse: () => widget.hub)!;
  }

  @override
  void initState() {
    super.initState();
    _authToken = widget.hub.token;
    unawaited(_loadAuthStatus());
  }

  Future<RhythmAuthApi> _authApi({bool localOnly = false}) async {
    final resolved = localOnly
        ? ServerEndpointResolver.local(_currentHub)
        : await ServerEndpointResolver.resolve(
            _currentHub,
            syncProvider: context.read<ServerSyncProvider>(),
          );
    return resolved.authApi(authToken: _authToken);
  }

  Future<void> _loadAuthStatus() async {
    setState(() => _isAuthLoading = true);
    try {
      final status = await (await _authApi()).getStatus();
      if (!mounted) return;
      setState(() {
        _authStatus = status;
        _isAuthLoading = false;
      });
    } catch (_) {
      if (!mounted) return;
      setState(() {
        _authStatus = null;
        _isAuthLoading = false;
      });
    }
  }

  Future<void> _setRemoteAccessEnabled(bool enabled) async {
    if (_isRemoteAccessUpdating) return;

    final remoteAccess = RemoteAccessService.instance;
    if (enabled && !remoteAccess.canUseRemoteAccess) {
      await PlanTierModal.show(context,
          highlightFeature: Entitlement.remoteAccess);
      return;
    }

    final hub = _currentHub;
    final homeProvider = context.read<HomeProvider>();
    final syncProvider = context.read<ServerSyncProvider>();
    setState(() => _isRemoteAccessUpdating = true);
    try {
      if (enabled) {
        final tokenHub =
            await _ensureOwnerTokenForRemoteAccess(hub, homeProvider);
        final result = await remoteAccess.enableForHub(
          tokenHub,
          home: homeProvider.currentHome,
        );
        await homeProvider.updateHub(result.updatedHub);
        _showSnackBar('Remote access enabled.');
      } else {
        final updatedHub = await remoteAccess.disableForHub(
          hub,
          home: homeProvider.currentHome,
        );
        await homeProvider.updateHub(updatedHub);
        _showSnackBar('Remote access disabled.');
      }
      syncProvider.connectIfAvailable();
    } catch (error) {
      _showSnackBar(
        enabled && error is RemoteAccessActivationException
            ? 'Remote access tunnel is still starting.'
            : enabled
                ? 'Could not enable remote access.'
                : 'Could not disable remote access.',
      );
      debugPrint('Remote access update failed: $error');
    } finally {
      if (mounted) {
        setState(() => _isRemoteAccessUpdating = false);
      }
    }
  }

  Future<Hub> _ensureOwnerTokenForRemoteAccess(
    Hub hub,
    HomeProvider homeProvider,
  ) async {
    final existingToken = hub.token?.trim();
    if (existingToken != null && existingToken.isNotEmpty) {
      _authToken = existingToken;
      return hub;
    }

    final claim = await (await _authApi(localOnly: true)).claimOwnerToken();
    final token = claim.token.trim();
    if (token.isEmpty) {
      throw StateError('Server returned an empty owner token.');
    }

    _authToken = token;
    final updatedHub = hub.copyWith(token: token);
    final saved = await homeProvider.updateHub(updatedHub);
    if (!saved && mounted) {
      _showSnackBar('Owner token created, but could not be saved.');
    }
    return updatedHub;
  }

  void _showSnackBar(String message) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(message),
        behavior: SnackBarBehavior.floating,
      ),
    );
  }

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
                    if (FeatureFlags.remoteAccessTunnel) ...[
                      _buildRemoteAccessSection(),
                      const SizedBox(height: 16),
                    ],
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

  Widget _buildRemoteAccessSection() {
    context.watch<HomeProvider>();
    final hub = _currentHub;
    final configured = hub.remoteEndpoint != null;
    final canCreateToken = _authStatus?.claimAvailable ?? false;
    final prerequisitesMet = _hasAuthToken || canCreateToken;
    final statusText = _isRemoteAccessUpdating
        ? 'Saving'
        : configured
            ? 'Enabled'
            : _isAuthLoading
                ? 'Checking'
                : _authStatus == null
                    ? 'Unavailable'
                    : !_hasAuthToken && canCreateToken
                        ? 'Ready'
                        : !_hasAuthToken
                            ? 'Token unavailable'
                            : 'Off';
    final statusColor = configured
        ? _enabledGreen
        : prerequisitesMet
            ? _warningAmber
            : CelestialColors.textSecondary;
    final canEnable =
        !_isRemoteAccessUpdating && !_isAuthLoading && prerequisitesMet;
    final canDisable = !_isRemoteAccessUpdating && configured;
    final canToggle = configured ? canDisable : canEnable;

    return _buildSection(
      title: 'REMOTE ACCESS',
      children: [
        _buildSwitchRow(
          icon: Icons.cloud_outlined,
          iconColor: statusColor,
          label: 'Remote Access',
          tooltip:
              'Uses a secure outbound tunnel so this server can be controlled away from home.',
          statusText: statusText,
          statusColor: statusColor,
          value: configured,
          activeTrackColor: _teal,
          busy: _isRemoteAccessUpdating,
          onChanged: canToggle
              ? (next) => unawaited(_setRemoteAccessEnabled(next))
              : null,
        ),
      ],
    );
  }

  Widget _buildAutomaticLightingSection() {
    final syncProvider = context.watch<ServerSyncProvider>();
    final connected =
        syncProvider.connectionState == RhythmConnectionState.connected;
    final enabled = syncProvider.lightBreakerEnabled;
    final statusText = connected ? (enabled ? 'On' : 'Off') : 'Offline';
    final statusColor = !connected
        ? CelestialColors.textSecondary.withValues(alpha: 0.6)
        : enabled
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
              ? (next) =>
                  unawaited(syncProvider.setLightBreakerEnabled(next))
              : null,
        ),
      ],
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
  const RhythmServerHubManagementSection({super.key});

  @override
  State<RhythmServerHubManagementSection> createState() =>
      _RhythmServerHubManagementSectionState();
}

class _RhythmServerHubManagementSectionState
    extends State<RhythmServerHubManagementSection> {
  bool _isConfiguringHub = false;
  bool _isFetchingSummaries = false;
  bool _hubSummariesLoaded = false;
  Map<String, String> _hubSummaries = {};

  bool get _isHaAddon =>
      context.read<ServerSyncProvider>().serverPlatformContext == 'ha_addon';

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
    final configuredSection = _buildConfiguredHubsSection(configuredHubs);
    if (configuredSection != null) {
      sections.add(configuredSection);
    }

    final addHubSection =
        _buildHubPairingSuggestions(syncProvider, configuredHubs);
    if (addHubSection != null) {
      if (sections.isNotEmpty) {
        sections.add(const SizedBox(height: 12));
      }
      sections.add(addHubSection);
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
    final configuredTypes = syncProvider.configuredHubTypes;
    final hasAnyHub = configuredHubs.isNotEmpty;
    final haConfigured = configuredTypes.contains('homeassistant') ||
        configuredTypes.contains('home_assistant');
    final showHa =
        syncProvider.canConfigureHub('homeassistant') && !haConfigured;
    final showHue =
        syncProvider.canConfigureHub('hue') && !configuredTypes.contains('hue');
    final matterOptions = _buildMatterAddOptionRows(syncProvider);
    final hasMatterOptions = matterOptions.isNotEmpty;

    if (!showHa && !showHue && !hasMatterOptions && !hasAnyHub) {
      return null;
    }

    if (!hasAnyHub) {
      return _buildSection(
        title: 'LIGHT HUB',
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
                _buildConnectionRow(
                  label: 'Status',
                  statusText: 'Not paired',
                  statusColor: CelestialColors.textSecondary,
                ),
                if (showHa) ...[
                  Divider(
                    height: 1,
                    color: CelestialColors.orbitRing.withValues(alpha: 0.3),
                  ),
                  _buildHubOptionRow(
                    icon: Icons.home_outlined,
                    label: 'Home Assistant',
                    color: const Color(0xFF42A5F5),
                    isLoading: _isConfiguringHub,
                    showBetaBadge: true,
                    onTap:
                        _isConfiguringHub ? null : () => _pairHa(syncProvider),
                  ),
                ],
                if (showHue) ...[
                  Divider(
                    height: 1,
                    color: CelestialColors.orbitRing.withValues(alpha: 0.3),
                  ),
                  _buildHubOptionRow(
                    icon: Icons.lightbulb_outline,
                    label: 'Philips Hue',
                    color: const Color(0xFFFFB900),
                    onTap: () => HueConfiguratorScreen.show(context),
                  ),
                ],
                if (hasMatterOptions) ...[
                  Divider(
                    height: 1,
                    color: CelestialColors.orbitRing.withValues(alpha: 0.3),
                  ),
                  ...matterOptions,
                ],
              ],
            ),
          ),
        ],
      );
    }

    final options = <Widget>[];
    if (showHa) {
      options.add(
        _buildHubOptionRow(
          icon: Icons.home_outlined,
          label: 'Add Home Assistant',
          color: const Color(0xFF42A5F5),
          isLoading: _isConfiguringHub,
          showBetaBadge: true,
          onTap: _isConfiguringHub ? null : () => _pairHa(syncProvider),
        ),
      );
    }
    if (showHue) {
      if (options.isNotEmpty) {
        options.add(
          Divider(
            height: 1,
            color: CelestialColors.orbitRing.withValues(alpha: 0.3),
          ),
        );
      }
      options.add(
        _buildHubOptionRow(
          icon: Icons.lightbulb_outline,
          label: 'Add Philips Hue',
          color: const Color(0xFFFFB900),
          onTap: () => HueConfiguratorScreen.show(context),
        ),
      );
    }
    if (hasMatterOptions) {
      if (options.isNotEmpty) {
        options.add(
          Divider(
            height: 1,
            color: CelestialColors.orbitRing.withValues(alpha: 0.3),
          ),
        );
      }
      options.addAll(matterOptions);
    }

    if (options.isEmpty) return null;

    return _buildSection(
      title: 'ADD HUB',
      children: [
        Container(
          decoration: BoxDecoration(
            color: CelestialColors.backgroundCard,
            borderRadius: BorderRadius.circular(14),
            border: Border.all(
              color: CelestialColors.orbitRing.withValues(alpha: 0.5),
            ),
          ),
          child: Column(children: options),
        ),
      ],
    );
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

  Future<void> _pairHa(ServerSyncProvider syncProvider) async {
    if (_isHaAddon) {
      setState(() => _isConfiguringHub = true);
      try {
        await syncProvider.configureAddonHaHub();
      } finally {
        if (mounted) setState(() => _isConfiguringHub = false);
      }
      return;
    }

    final result = await HAConfiguratorScreen.show(context);
    if (result == true && mounted) {
      await syncProvider.pushHubCredentials(RoomSourceDto.homeAssistant);
    }
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

  Widget _buildConnectionRow({
    required String label,
    required String statusText,
    required Color statusColor,
    bool isPulsing = false,
    bool isSpinning = false,
  }) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
      child: Row(
        children: [
          SizedBox(
            width: 80,
            child: Text(
              label,
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.8),
                fontSize: 14,
              ),
            ),
          ),
          Container(
            width: 8,
            height: 8,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: statusColor,
              boxShadow: isPulsing
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
          const SizedBox(width: 8),
          Expanded(
            child: Text(
              statusText,
              style: TextStyle(
                color: statusColor,
                fontSize: 14,
                fontWeight: FontWeight.w500,
              ),
            ),
          ),
          if (isSpinning)
            SizedBox(
              width: 14,
              height: 14,
              child: CircularProgressIndicator(
                strokeWidth: 1.5,
                valueColor: AlwaysStoppedAnimation(
                  CelestialColors.textSecondary.withValues(alpha: 0.5),
                ),
              ),
            ),
        ],
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
    VoidCallback? onTap,
  }) {
    return GestureDetector(
      onTap: onTap,
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
            else if (onTap != null)
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
// OTA Update Overlay (full-screen blocking during firmware update)
// =============================================================================

class _OtaUpdateOverlay extends StatefulWidget {
  final OtaService otaService;
  final RhythmConnection? connection;

  const _OtaUpdateOverlay({required this.otaService, this.connection});

  static Future<void> show(
    BuildContext context, {
    required OtaService otaService,
    RhythmConnection? connection,
  }) {
    return Navigator.of(context, rootNavigator: true).push(
      PageRouteBuilder(
        opaque: true,
        pageBuilder: (_, __, ___) => _OtaUpdateOverlay(
          otaService: otaService,
          connection: connection,
        ),
        transitionsBuilder: (_, animation, __, child) {
          return FadeTransition(opacity: animation, child: child);
        },
        transitionDuration: const Duration(milliseconds: 300),
      ),
    );
  }

  @override
  State<_OtaUpdateOverlay> createState() => _OtaUpdateOverlayState();
}

class _OtaUpdateOverlayState extends State<_OtaUpdateOverlay>
    with TickerProviderStateMixin {
  late AnimationController _pulseController;
  late Animation<double> _pulseAnimation;
  late AnimationController _spinController;
  String? _targetVersionLabel;
  bool _isBundleRepair = false;
  StreamSubscription<RhythmOtaUpdateProgress>? _otaProgressSub;
  StreamSubscription<RhythmConnectionState>? _connectionStateSub;
  RhythmOtaUpdateProgress? _latestProgress;
  RhythmConnectionState? _connectionState;
  bool _awaitingPostOtaConnection = false;
  bool _sawPostOtaConnected = false;

  static const _teal = Color(0xFF00BCD4);

  @override
  void initState() {
    super.initState();
    widget.otaService.addListener(_onStateChanged);
    _syncOverlayMetadata();
    _syncPostOtaConnectionGate();

    _pulseController = AnimationController(
      duration: const Duration(milliseconds: 2000),
      vsync: this,
    )..repeat(reverse: true);

    _pulseAnimation = Tween<double>(begin: 0.3, end: 0.8).animate(
      CurvedAnimation(parent: _pulseController, curve: Curves.easeInOut),
    );

    // Continuous clockwise spin for the circular-arrow hero icon
    // (Restarting / reconnecting stages).
    _spinController = AnimationController(
      duration: const Duration(milliseconds: 1100),
      vsync: this,
    )..repeat();

    _otaProgressSub =
        widget.connection?.otaUpdateProgressEvents.listen((event) {
      if (!mounted) return;
      setState(() {
        _latestProgress = event;
      });
    });

    _connectionState = widget.connection?.connectionState;
    _connectionStateSub =
        widget.connection?.connectionStateStream.listen((state) {
      if (!mounted) return;
      setState(() {
        _connectionState = state;
        if (_awaitingPostOtaConnection &&
            state == RhythmConnectionState.connected) {
          _sawPostOtaConnected = true;
        }
      });
    });
  }

  @override
  void dispose() {
    widget.otaService.removeListener(_onStateChanged);
    _otaProgressSub?.cancel();
    _connectionStateSub?.cancel();
    _pulseController.dispose();
    _spinController.dispose();
    super.dispose();
  }

  void _onStateChanged() {
    _syncOverlayMetadata();
    _syncPostOtaConnectionGate();
    if (mounted) setState(() {});
  }

  void _syncOverlayMetadata() {
    _isBundleRepair = _isBundleRepair || widget.otaService.isBundleRepair;
    _targetVersionLabel ??= _resolveTargetVersionLabel();
  }

  String? _resolveTargetVersionLabel() {
    final releaseVersion = widget.otaService.availableRelease?.version;
    if (releaseVersion != null && releaseVersion.isNotEmpty) {
      return _formatOtaVersionLabel(releaseVersion);
    }

    final latestVersion = widget.otaService.latestVersion;
    if (latestVersion != null && latestVersion.isNotEmpty) {
      return _formatOtaVersionLabel(latestVersion);
    }

    return null;
  }

  void _syncPostOtaConnectionGate() {
    if (widget.connection == null) {
      _sawPostOtaConnected = true;
      return;
    }
    if (widget.otaService.state == OtaState.complete &&
        !_awaitingPostOtaConnection) {
      _awaitingPostOtaConnection = true;
      _sawPostOtaConnected = false;
    }
  }

  String _buildProgressMessage({required bool isDownloading}) {
    final targetVersion = _targetVersionLabel;
    if (targetVersion == null) {
      return isDownloading ? 'Downloading update...' : 'Installing update...';
    }

    if (_isBundleRepair) {
      return isDownloading
          ? 'Downloading repair for $targetVersion...'
          : 'Installing repair for $targetVersion...';
    }

    return isDownloading
        ? 'Downloading update to $targetVersion...'
        : 'Installing update to $targetVersion...';
  }

  String _buildRebootingMessage() {
    final targetVersion = _targetVersionLabel;
    if (targetVersion == null) {
      return 'Restarting your device...';
    }

    if (_isBundleRepair) {
      return 'Restarting after repair on $targetVersion...';
    }

    return 'Restarting into $targetVersion...';
  }

  String _buildCompletionMessage() {
    final statusMessage = widget.otaService.statusMessage?.trim();
    if (statusMessage != null && statusMessage.isNotEmpty) {
      return statusMessage;
    }

    final targetVersion = _targetVersionLabel;
    if (targetVersion == null) {
      return 'Your device is up to date.';
    }

    return _isBundleRepair
        ? 'Repair completed on $targetVersion.'
        : 'Updated to $targetVersion.';
  }

  bool get _isFinished {
    final state = widget.otaService.state;
    return state == OtaState.error ||
        (state == OtaState.complete && _serverReady);
  }

  bool get _serverReady {
    final connection = widget.connection;
    if (connection == null) return true;
    if (widget.otaService.state == OtaState.complete) {
      return _sawPostOtaConnected;
    }
    return (_connectionState ?? connection.connectionState) ==
        RhythmConnectionState.connected;
  }

  bool get _waitingForServerReady =>
      widget.otaService.state == OtaState.complete && !_serverReady;

  void _dismiss() {
    Navigator.of(context).pop();
  }

  @override
  Widget build(BuildContext context) {
    return PopScope(
      canPop: false,
      onPopInvokedWithResult: (didPop, _) {
        if (!didPop && _isFinished) {
          _dismiss();
        }
      },
      child: Scaffold(
        backgroundColor: CelestialColors.backgroundDark,
        body: SafeArea(
          child: Center(
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 40),
              child: _buildContent(),
            ),
          ),
        ),
      ),
    );
  }

  Widget _buildContent() {
    switch (widget.otaService.state) {
      case OtaState.complete:
        if (_waitingForServerReady) {
          return _buildStagedProgress(waitingForServerReady: true);
        }
        return _buildComplete();
      case OtaState.error:
        return _buildError();
      default:
        return _buildStagedProgress();
    }
  }

  /// 5-stage timeline for the server self-update flow:
  ///   0 = Checking, 1 = Downloading (with %), 2 = Verifying,
  ///   3 = Installing, 4 = Restarting.
  int _otaActiveStageIndex() {
    final progress = _latestProgress;
    if (progress != null) {
      final mapped = switch (progress.stage) {
        RhythmOtaUpdateStage.checking => 0,
        RhythmOtaUpdateStage.updateAvailable => 0,
        RhythmOtaUpdateStage.upToDate => 0,
        RhythmOtaUpdateStage.downloading => 1,
        RhythmOtaUpdateStage.verifying => 2,
        RhythmOtaUpdateStage.staging => 3,
        RhythmOtaUpdateStage.installing => 3,
        RhythmOtaUpdateStage.finalizing => 3,
        RhythmOtaUpdateStage.restarting => 4,
        // Server doesn't echo which stage failed, so attribute it to whatever
        // we last knew about — that gives the timeline a sensible failed dot.
        RhythmOtaUpdateStage.failed => _lastObservedActiveIndex,
      };
      _lastObservedActiveIndex = mapped;
      return mapped;
    }
    // Fall back to OtaService coarse state.
    final fromCoarse = switch (widget.otaService.state) {
      OtaState.checking => 0,
      OtaState.downloading => 1,
      OtaState.uploading => 3,
      OtaState.flashing => 3,
      OtaState.rebooting => 4,
      _ => 0,
    };
    _lastObservedActiveIndex = fromCoarse;
    return fromCoarse;
  }

  int _lastObservedActiveIndex = 0;

  String? _otaActiveMessage(int activeIndex) {
    final progress = _latestProgress;
    if (progress != null && progress.message.trim().isNotEmpty) {
      return progress.message.trim();
    }
    // Sensible fallbacks per stage when no SSE event has arrived yet.
    return switch (activeIndex) {
      0 => 'Checking for updates',
      1 => _buildProgressMessage(isDownloading: true),
      2 => 'Verifying download integrity',
      3 => _buildProgressMessage(isDownloading: false),
      4 => _buildRebootingMessage(),
      _ => null,
    };
  }

  String _otaTitle(int activeIndex) {
    if (activeIndex == 4) return 'Restarting Server';
    return 'Updating Server';
  }

  IconData _otaHeroIcon(int activeIndex) {
    return switch (activeIndex) {
      0 => Icons.search_rounded,
      1 => Icons.cloud_download_outlined,
      2 => Icons.verified_outlined,
      3 => Icons.system_update,
      4 => Icons.restart_alt_rounded,
      _ => Icons.system_update,
    };
  }

  /// Format `total / downloaded` bytes as e.g. `45.2 / 62.8 MB`.
  String? _bytesLabel() {
    final p = _latestProgress;
    if (p == null) return null;
    final downloaded = p.downloadedBytes;
    final total = p.totalBytes;
    if (downloaded == null) return null;
    if (total == null || total <= 0) {
      return _formatBytes(downloaded);
    }
    final d = _formatBytesValue(downloaded, total);
    final t = _formatBytes(total);
    return '$d / $t';
  }

  static const _kb = 1024;
  static const _mb = 1024 * 1024;
  static const _gb = 1024 * 1024 * 1024;

  static String _formatBytes(int bytes) {
    if (bytes >= _gb) return '${(bytes / _gb).toStringAsFixed(2)} GB';
    if (bytes >= _mb) return '${(bytes / _mb).toStringAsFixed(1)} MB';
    if (bytes >= _kb) return '${(bytes / _kb).toStringAsFixed(0)} KB';
    return '$bytes B';
  }

  static String _formatBytesValue(int value, int total) {
    if (total >= _gb) return (value / _gb).toStringAsFixed(2);
    if (total >= _mb) return (value / _mb).toStringAsFixed(1);
    if (total >= _kb) return (value / _kb).toStringAsFixed(0);
    return value.toString();
  }

  Widget _buildStagedProgress({bool waitingForServerReady = false}) {
    final activeIndex = waitingForServerReady ? 4 : _otaActiveStageIndex();
    final activeMessage = waitingForServerReady
        ? 'Waiting for the server connection to finish restoring'
        : _otaActiveMessage(activeIndex);
    final title =
        waitingForServerReady ? 'Reconnecting Server' : _otaTitle(activeIndex);
    final heroIcon =
        waitingForServerReady ? Icons.sync_rounded : _otaHeroIcon(activeIndex);
    // The Restarting / reconnecting stages use a circular-arrow icon — spin it.
    final spinHero = waitingForServerReady || activeIndex == 4;
    final percent = !waitingForServerReady && activeIndex == 1
        ? _latestProgress?.percent
        : null;
    final bytesLabel =
        !waitingForServerReady && activeIndex == 1 ? _bytesLabel() : null;

    return SingleChildScrollView(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Center(
            child: AnimatedBuilder(
              animation: _pulseAnimation,
              builder: (context, _) {
                return Container(
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
                            alpha: _pulseAnimation.value * 0.28),
                        blurRadius: 32,
                        spreadRadius: 4,
                      ),
                    ],
                  ),
                  child: spinHero
                      ? RotationTransition(
                          turns: _spinController,
                          child: Icon(heroIcon, color: _teal, size: 38),
                        )
                      : Icon(heroIcon, color: _teal, size: 38),
                );
              },
            ),
          ),
          const SizedBox(height: 28),
          Text(
            title,
            textAlign: TextAlign.center,
            style: const TextStyle(
              color: CelestialColors.textPrimary,
              fontSize: 22,
              fontWeight: FontWeight.w600,
              letterSpacing: 0.2,
            ),
          ),
          if (_targetVersionLabel != null) ...[
            const SizedBox(height: 4),
            Text(
              _isBundleRepair
                  ? 'Repairing $_targetVersionLabel'
                  : 'Installing $_targetVersionLabel',
              textAlign: TextAlign.center,
              style: TextStyle(
                color: _teal.withValues(alpha: 0.85),
                fontSize: 13,
                fontWeight: FontWeight.w500,
                fontFamily: 'monospace',
                letterSpacing: 0.4,
              ),
            ),
          ],
          const SizedBox(height: 28),
          Container(
            padding: const EdgeInsets.fromLTRB(20, 22, 20, 22),
            decoration: BoxDecoration(
              color: CelestialColors.backgroundCard.withValues(alpha: 0.6),
              borderRadius: BorderRadius.circular(18),
              border: Border.all(
                color: _teal.withValues(alpha: 0.12),
                width: 1,
              ),
            ),
            child: StageTimeline(
              stages: const [
                StageTimelineItem(
                  label: 'Checking for updates',
                  icon: Icons.search_rounded,
                ),
                StageTimelineItem(
                  label: 'Downloading',
                  icon: Icons.cloud_download_outlined,
                ),
                StageTimelineItem(
                  label: 'Verifying',
                  icon: Icons.fingerprint_rounded,
                ),
                StageTimelineItem(
                  label: 'Installing',
                  icon: Icons.settings_suggest_outlined,
                ),
                StageTimelineItem(
                  label: 'Restarting',
                  icon: Icons.restart_alt_rounded,
                ),
              ],
              activeIndex: activeIndex,
              activeMessage: activeMessage,
              activePercent: percent,
              activeBytesLabel: bytesLabel,
              failed: !waitingForServerReady &&
                  _latestProgress?.stage == RhythmOtaUpdateStage.failed,
              accent: _teal,
            ),
          ),
          const SizedBox(height: 24),
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
      ),
    );
  }

  Widget _buildComplete() {
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        Container(
          width: 80,
          height: 80,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: const Color(0xFF22C55E).withValues(alpha: 0.15),
            boxShadow: [
              BoxShadow(
                color: const Color(0xFF22C55E).withValues(alpha: 0.2),
                blurRadius: 24,
                spreadRadius: 4,
              ),
            ],
          ),
          child: const Icon(
            Icons.check_rounded,
            color: Color(0xFF22C55E),
            size: 40,
          ),
        ),
        const SizedBox(height: 32),
        const Text(
          'Update Complete',
          style: TextStyle(
            color: CelestialColors.textPrimary,
            fontSize: 22,
            fontWeight: FontWeight.w600,
          ),
        ),
        const SizedBox(height: 8),
        Text(
          _buildCompletionMessage(),
          style: const TextStyle(
            color: Color(0xFF22C55E),
            fontSize: 15,
            fontWeight: FontWeight.w500,
          ),
        ),
        const SizedBox(height: 40),
        GestureDetector(
          onTap: _dismiss,
          child: Container(
            width: double.infinity,
            padding: const EdgeInsets.symmetric(vertical: 14),
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(14),
              color: _teal.withValues(alpha: 0.1),
              border: Border.all(
                color: _teal.withValues(alpha: 0.3),
              ),
            ),
            child: const Text(
              'Done',
              textAlign: TextAlign.center,
              style: TextStyle(
                color: _teal,
                fontSize: 16,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
        ),
      ],
    );
  }

  Widget _buildError() {
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
          'Update Failed',
          style: TextStyle(
            color: CelestialColors.textPrimary,
            fontSize: 22,
            fontWeight: FontWeight.w600,
          ),
        ),
        const SizedBox(height: 8),
        Text(
          'The update did not finish. Please try again.',
          textAlign: TextAlign.center,
          style: TextStyle(
            color: Colors.red.shade400,
            fontSize: 14,
          ),
        ),
        const SizedBox(height: 40),
        GestureDetector(
          onTap: _dismiss,
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
  int _selectedTab = 0; // 0 = Overview, 1 = Logs

  // Vitals
  Map<String, dynamic>? _vitals;
  bool _loadingVitals = true;

  // Logs
  List<Map<String, dynamic>>? _logs;
  bool _loadingLogs = true;
  String? _categoryFilter;

  static const _teal = Color(0xFF00BCD4);
  static const _logCategories = ['all', 'conn', 'hub', 'cmd', 'evt', 'sys'];

  @override
  void initState() {
    super.initState();
    _loadInitialData();
  }

  Future<void> _loadInitialData() async {
    await _loadVitals();
    _fetchLogs(); // after vitals completes — avoid 2 concurrent HTTP requests
  }

  Future<void> _loadVitals() async {
    final vitals = await widget.client.getDiagVitals();
    if (mounted) {
      setState(() {
        _vitals = vitals;
        _loadingVitals = false;
      });
    }
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
    if (_selectedTab == 0) {
      setState(() => _loadingVitals = true);
      await _loadVitals();
    } else {
      await _fetchLogs();
    }
  }

  Future<void> _handleDismissCrash() async {
    final cleared = await widget.client.clearCrashInfo();
    if (cleared && mounted) {
      _loadVitals();
    }
  }

  // ─── Formatters ──────────────────────────────────────────

  String _formatUptime(dynamic secs) {
    final s = (secs is num) ? secs.toInt() : 0;
    if (s <= 0) return 'Unknown';
    final days = s ~/ 86400;
    final hours = (s % 86400) ~/ 3600;
    final mins = (s % 3600) ~/ 60;
    final sec = s % 60;
    if (days > 0) return '${days}d ${hours}h ${mins}m';
    if (hours > 0) return '${hours}h ${mins}m';
    return '${mins}m ${sec}s';
  }

  String _formatMemory(dynamic bytes) {
    final b = (bytes is num) ? bytes.toInt() : 0;
    if (b <= 0) return 'Unknown';
    return '${(b / 1024).round()} KB';
  }

  String _formatSecsAgo(dynamic secs) {
    if (secs == null) return 'Never';
    final s = (secs is num) ? secs.toInt() : 0;
    if (s < 60) return '${s}s ago';
    if (s < 3600) return '${s ~/ 60}m ago';
    return '${s ~/ 3600}h ago';
  }

  String _formatResetReason(dynamic reason) {
    final s = reason as String? ?? 'unknown';
    switch (s) {
      case 'power_on':
        return 'Power On';
      case 'software':
        return 'Software';
      case 'panic':
        return 'Panic';
      case 'int_wdt':
        return 'Interrupt WDT';
      case 'task_wdt':
        return 'Task WDT';
      case 'wdt':
        return 'Watchdog';
      case 'brownout':
        return 'Brownout';
      default:
        return s[0].toUpperCase() + s.substring(1);
    }
  }

  String _formatCrashTimestamp(dynamic ts) {
    if (ts == null) return 'Unknown';
    final epoch = (ts is num) ? ts.toInt() : 0;
    if (epoch == 0) return 'Unknown';
    final dt = DateTime.fromMillisecondsSinceEpoch(epoch * 1000);
    final now = DateTime.now();
    final diff = now.difference(dt);
    if (diff.inMinutes < 1) return 'Just now';
    if (diff.inHours < 1) return '${diff.inMinutes}m ago';
    if (diff.inDays < 1) return '${diff.inHours}h ago';
    if (diff.inDays < 7) return '${diff.inDays}d ago';
    return '${dt.month}/${dt.day}';
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
    final isLoading = _selectedTab == 0 ? _loadingVitals : _loadingLogs;

    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            _buildHeader(isLoading),
            _buildTabToggle(),
            Container(
              height: 1,
              color: CelestialColors.orbitRing.withValues(alpha: 0.2),
            ),
            Expanded(
              child: _selectedTab == 0 ? _buildOverviewTab() : _buildLogsTab(),
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

  Widget _buildTabToggle() {
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 0, 16, 12),
      child: Container(
        height: 36,
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(10),
          color: Colors.white.withValues(alpha: 0.04),
          border: Border.all(
            color: CelestialColors.orbitRing.withValues(alpha: 0.3),
          ),
        ),
        child: Row(
          children: [
            _buildTab('Overview', 0),
            _buildTab('Logs', 1),
          ],
        ),
      ),
    );
  }

  Widget _buildTab(String label, int index) {
    final isSelected = _selectedTab == index;
    return Expanded(
      child: GestureDetector(
        onTap: () => setState(() => _selectedTab = index),
        child: Container(
          margin: const EdgeInsets.all(3),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(7),
            color:
                isSelected ? _teal.withValues(alpha: 0.2) : Colors.transparent,
          ),
          child: Center(
            child: Text(
              label,
              style: TextStyle(
                color: isSelected ? _teal : CelestialColors.textSecondary,
                fontSize: 13,
                fontWeight: isSelected ? FontWeight.w600 : FontWeight.w500,
                letterSpacing: 0.3,
              ),
            ),
          ),
        ),
      ),
    );
  }

  // ─── Overview tab ─────────────────────────────────────────

  Widget _buildOverviewTab() {
    if (_loadingVitals) {
      return const Center(
        child: SizedBox(
          width: 20,
          height: 20,
          child: CircularProgressIndicator(
            strokeWidth: 2,
            valueColor: AlwaysStoppedAnimation(_teal),
          ),
        ),
      );
    }

    if (_vitals == null) {
      return Center(
        child: Text(
          'Not available',
          style: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.6),
            fontSize: 14,
          ),
        ),
      );
    }

    final v = _vitals!;
    final hubs = v['hubs'] as List<dynamic>? ?? [];

    return SingleChildScrollView(
      padding: const EdgeInsets.fromLTRB(16, 16, 16, 32),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          // Crash card (shown first when crashes recorded)
          if (((v['crash_count'] as num?)?.toInt() ?? 0) > 0) ...[
            _buildCrashDiagCard(v),
            const SizedBox(height: 10),
          ],
          // System sub-card
          _buildDiagCard(
            title: 'System',
            rows: [
              _buildDiagRow('Uptime', _formatUptime(v['uptime_secs'])),
              _buildDiagRow('Free Memory', _formatMemory(v['free_heap_bytes'])),
              _buildDiagRow(
                'Min Free Memory',
                _formatMemory(v['min_free_heap_bytes']),
              ),
              _buildDiagRow(
                'Reset Reason',
                _formatResetReason(v['reset_reason']),
              ),
            ],
          ),
          if (hubs.isNotEmpty) ...[
            const SizedBox(height: 10),
            _buildHubDiagCard(hubs.first as Map<String, dynamic>),
          ],
          const SizedBox(height: 10),
          _buildCommandsDiagCard(v),
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

  // ─── Diagnostics cards ─────────────────────────────────────

  Widget _buildDiagCard({
    required String title,
    required List<Widget> rows,
  }) {
    return Container(
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(
          color: CelestialColors.orbitRing.withValues(alpha: 0.5),
        ),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 12, 16, 4),
            child: Text(
              title,
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.5),
                fontSize: 11,
                fontWeight: FontWeight.w600,
                letterSpacing: 0.8,
              ),
            ),
          ),
          ...rows,
          const SizedBox(height: 4),
        ],
      ),
    );
  }

  Widget _buildDiagRow(String label, String value, {Color? valueColor}) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 6),
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
          Text(
            value,
            style: TextStyle(
              color: valueColor ?? CelestialColors.textPrimary,
              fontSize: 14,
              fontWeight: FontWeight.w500,
              fontFamily: 'monospace',
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildDiagRowWithDot(String label, String value, Color dotColor) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 6),
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
          Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Container(
                width: 7,
                height: 7,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: dotColor,
                ),
              ),
              const SizedBox(width: 6),
              Text(
                value,
                style: TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 14,
                  fontWeight: FontWeight.w500,
                  fontFamily: 'monospace',
                ),
              ),
            ],
          ),
        ],
      ),
    );
  }

  Widget _buildHubDiagCard(Map<String, dynamic> hub) {
    final hubType = (hub['type'] as String? ?? 'unknown');
    final connState = hub['conn_state'] as String? ?? 'unknown';
    final heartbeatAgo = hub['last_heartbeat_secs_ago'];
    final reconnects = (hub['reconnect_count'] as num?)?.toInt() ?? 0;

    Color connColor;
    String connLabel;
    switch (connState) {
      case 'connected':
        connColor = const Color(0xFF22C55E);
        connLabel = 'Connected';
      case 'connecting':
        connColor = Colors.amber;
        connLabel = 'Connecting';
      default:
        connColor = Colors.red.shade400;
        connLabel = 'Disconnected';
    }

    return _buildDiagCard(
      title: 'Hub',
      rows: [
        _buildDiagRow(
          'Type',
          hubType[0].toUpperCase() + hubType.substring(1),
        ),
        _buildDiagRowWithDot('State', connLabel, connColor),
        _buildDiagRow('Last Heartbeat', _formatSecsAgo(heartbeatAgo)),
        _buildDiagRow(
          'Reconnects',
          '$reconnects',
          valueColor: reconnects > 0 ? Colors.amber : null,
        ),
      ],
    );
  }

  Widget _buildCommandsDiagCard(Map<String, dynamic> v) {
    final lastResult = v['last_cmd_result'] as String? ?? 'none';
    final lastSecsAgo = v['last_cmd_secs_ago'];
    final okCount = (v['cmd_ok_count'] as num?)?.toInt() ?? 0;
    final errCount = (v['cmd_err_count'] as num?)?.toInt() ?? 0;
    final total = okCount + errCount;

    String lastCmdText;
    Color? lastCmdColor;
    if (lastResult == 'none' || lastSecsAgo == null) {
      lastCmdText = 'None';
    } else {
      final resultLabel = lastResult == 'ok' ? 'OK' : 'Error';
      lastCmdColor =
          lastResult == 'ok' ? const Color(0xFF22C55E) : Colors.red.shade400;
      lastCmdText = '$resultLabel ${_formatSecsAgo(lastSecsAgo)}';
    }

    String rateText;
    Color? rateColor;
    if (total == 0) {
      rateText = 'No commands';
    } else {
      final pct = (okCount / total * 100);
      rateText =
          '${pct.toStringAsFixed(pct.truncateToDouble() == pct ? 0 : 1)}% ($okCount/$total)';
      if (pct >= 98) {
        rateColor = const Color(0xFF22C55E);
      } else if (pct >= 90) {
        rateColor = Colors.amber;
      } else {
        rateColor = Colors.red.shade400;
      }
    }

    return _buildDiagCard(
      title: 'Commands',
      rows: [
        _buildDiagRow('Last Command', lastCmdText, valueColor: lastCmdColor),
        _buildDiagRow('Success Rate', rateText, valueColor: rateColor),
      ],
    );
  }

  Widget _buildCrashDiagCard(Map<String, dynamic> v) {
    final crashCount = (v['crash_count'] as num?)?.toInt() ?? 0;
    final crashReason = v['last_crash_reset_reason'] as String?;
    final panicMsg = v['last_panic_msg'] as String?;
    final crashTs = v['last_crash_ts'];

    return Container(
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(
          color: Colors.red.shade400.withValues(alpha: 0.4),
        ),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 12, 16, 4),
            child: Row(
              children: [
                Icon(Icons.warning_amber_rounded,
                    color: Colors.red.shade400, size: 14),
                const SizedBox(width: 6),
                Text(
                  'LAST CRASH',
                  style: TextStyle(
                    color: Colors.red.shade400.withValues(alpha: 0.8),
                    fontSize: 11,
                    fontWeight: FontWeight.w600,
                    letterSpacing: 0.8,
                  ),
                ),
              ],
            ),
          ),
          _buildDiagRow(
            'Crash Count',
            '$crashCount',
            valueColor: Colors.red.shade400,
          ),
          if (crashReason != null)
            _buildDiagRow('Reason', _formatResetReason(crashReason)),
          _buildDiagRow('When', _formatCrashTimestamp(crashTs)),
          if (panicMsg != null) ...[
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 4, 16, 8),
              child: Container(
                width: double.infinity,
                padding: const EdgeInsets.all(10),
                decoration: BoxDecoration(
                  color: Colors.black.withValues(alpha: 0.3),
                  borderRadius: BorderRadius.circular(8),
                ),
                child: SelectableText(
                  panicMsg,
                  style: TextStyle(
                    color: Colors.red.shade300,
                    fontSize: 11,
                    fontFamily: 'monospace',
                    height: 1.4,
                  ),
                ),
              ),
            ),
          ],
          GestureDetector(
            onTap: _handleDismissCrash,
            child: Container(
              margin: const EdgeInsets.fromLTRB(12, 4, 12, 12),
              padding: const EdgeInsets.symmetric(vertical: 10),
              decoration: BoxDecoration(
                borderRadius: BorderRadius.circular(10),
                color: CelestialColors.textSecondary.withValues(alpha: 0.08),
                border: Border.all(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.2),
                ),
              ),
              child: Row(
                mainAxisAlignment: MainAxisAlignment.center,
                children: [
                  Icon(Icons.close_rounded,
                      color: CelestialColors.textSecondary, size: 16),
                  const SizedBox(width: 6),
                  Text(
                    'Dismiss',
                    style: TextStyle(
                      color: CelestialColors.textSecondary,
                      fontSize: 14,
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                ],
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
