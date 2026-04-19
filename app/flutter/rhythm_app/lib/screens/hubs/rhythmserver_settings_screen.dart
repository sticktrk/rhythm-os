import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show
        RhythmConnection,
        RhythmConnectionState,
        RhythmDevice,
        RhythmDeviceType,
        RhythmDiagnosticsApi,
        RhythmRoom,
        RoomModeState;
import '../../widgets/solar_orbit.dart';
import '../../providers/server_sync_provider.dart';
import '../../providers/home_provider.dart';
import '../../providers/room_provider.dart';
import '../../services/analytics_service.dart';
import '../../services/ota_service.dart';
import '../../widgets/device_detail_sheet.dart';
import 'ha_configurator_screen.dart';
import 'hue_configurator_screen.dart';
import 'matter_add_method.dart';
import 'matter_pairing_flow.dart';

/// Settings screen for a connected server hub (ESP32, standalone, HA addon).
///
/// Adapts visible sections based on the server's platform context.
class RhythmServerSettingsScreen extends StatefulWidget {
  final Hub hub;

  const RhythmServerSettingsScreen({super.key, required this.hub});

  /// Show as a full-screen modal with slide-up transition.
  static Future<void> show(BuildContext context, {required Hub hub}) {
    return Navigator.of(context).push(
      PageRouteBuilder(
        opaque: false,
        barrierColor: Colors.black54,
        pageBuilder: (context, animation, secondaryAnimation) {
          return RhythmServerSettingsScreen(hub: hub);
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
  late final RhythmDiagnosticsApi _client;
  final OtaService _otaService = OtaService();

  bool _isOnline = false;
  bool _checkingHealth = true;
  bool _isResetting = false;
  bool _isRebooting = false;
  bool _isFactoryResetting = false;
  bool _isRefreshing = false;
  bool _isConfiguringHub = false;
  OtaState? _lastHandledOtaState;

  // Per-hub-type device summaries from /api/devices/canonical
  Map<String, String> _hubSummaries = {};
  bool _hubSummariesLoaded = false;

  late AnimationController _glowController;
  late Animation<double> _glowAnimation;

  static const _teal = Color(0xFF00BCD4);
  static const _tealDeep = Color(0xFF0097A7);

  // ─── Server-type-aware computed getters ────────────────────

  String get _serverContext =>
      context.read<ServerSyncProvider>().serverPlatformContext;
  bool get _isEmbedded =>
      _serverContext == 'embedded' || _serverContext == 'esp32';
  bool get _isHaAddon => _serverContext == 'ha_addon';

  String get _headerTitle => switch (_serverContext) {
        'ha_addon' => 'Rhythm Add-on',
        'server' => 'Rhythm Server',
        _ => 'RhythmServer',
      };

  IconData get _heroIcon => switch (_serverContext) {
        'ha_addon' => Icons.home_outlined,
        'server' => Icons.dns_outlined,
        _ => Icons.developer_board,
      };

  @override
  void initState() {
    super.initState();
    _client = RhythmDiagnosticsApi(
        host: widget.hub.endpoint.host, port: widget.hub.endpoint.port);

    _glowController = AnimationController(
      duration: const Duration(milliseconds: 2000),
      vsync: this,
    )..repeat(reverse: true);

    _glowAnimation = Tween<double>(begin: 0.3, end: 0.6).animate(
      CurvedAnimation(parent: _glowController, curve: Curves.easeInOut),
    );

    _otaService.addListener(_onOtaStateChanged);
    _checkHealth();
    _fetchHubSummaries();
    unawaited(_loadOtaSupport());

    AnalyticsService().logScreenView('rhythmserver_settings');
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
      context.read<ServerSyncProvider>().connection.reconnect();
    } else if (_otaService.state == OtaState.error) {
      AnalyticsService().logOtaUpdateFailed(
        _otaService.errorMessage ?? 'Unknown error',
      );
    }

    setState(() {});
  }

  Future<void> _loadOtaSupport() async {
    final syncProvider = context.read<ServerSyncProvider>();
    await _otaService.initialize(
      host: widget.hub.endpoint.host,
      port: widget.hub.endpoint.port,
      fallbackCurrentVersion: syncProvider.firmwareVersion,
      fallbackPlatformType: syncProvider.serverPlatformType,
      fallbackPlatformContext: syncProvider.serverPlatformContext,
    );
  }

  Future<void> _checkHealth() async {
    final online = await _client.healthCheck();
    if (mounted) {
      setState(() {
        _isOnline = online;
        _checkingHealth = false;
      });
    }
  }

  Future<void> _fetchHubSummaries() async {
    final http = context.read<RhythmConnection>();
    final devices = await http.api.getCanonicalDevices();
    if (!mounted) return;
    if (devices == null) return;

    // Count devices per hub type from canonical endpoints.
    final counts = <String, _DeviceCounts>{};
    for (final d in devices) {
      final dtype = d['device_type'] as String? ?? 'light';
      final endpoints = d['endpoints'] as List<dynamic>? ?? [];
      final hubTypes = <String>{};
      for (final ep in endpoints) {
        final hubKey =
            (ep as Map<String, dynamic>)['hub_key'] as Map<String, dynamic>? ??
                {};
        final ht = hubKey['hub_type']?.toString();
        if (ht != null) hubTypes.add(ht);
      }
      for (final ht in hubTypes) {
        final c = counts[ht] ??= _DeviceCounts();
        if (dtype == 'light') {
          c.lights++;
        } else if (dtype == 'button') {
          c.buttons++;
        } else if (dtype == 'motion') {
          c.motion++;
        }
      }
    }

    final summaries = <String, String>{};
    for (final entry in counts.entries) {
      final c = entry.value;
      final parts = <String>[];
      if (c.lights > 0) {
        parts.add('${c.lights} light${c.lights > 1 ? 's' : ''}');
      }
      if (c.buttons > 0) {
        parts.add('${c.buttons} button${c.buttons > 1 ? 's' : ''}');
      }
      if (c.motion > 0) {
        parts.add('${c.motion} sensor${c.motion > 1 ? 's' : ''}');
      }
      summaries[entry.key] = parts.isEmpty ? 'No devices' : parts.join(', ');
    }

    setState(() {
      _hubSummaries = summaries;
      _hubSummariesLoaded = true;
    });
  }

  Future<void> _handleRefresh() async {
    if (_isRefreshing) return;
    setState(() {
      _isRefreshing = true;
      _checkingHealth = true;
    });

    // Refresh everything in parallel: health, server state, matter devices.
    await Future.wait([
      _checkHealth(),
      _fetchHubSummaries(),
      _loadOtaSupport(),
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

  Future<void> _handleReset() async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: Text(
          'Disconnect $_headerTitle',
          style: const TextStyle(color: CelestialColors.textPrimary),
        ),
        content: Text(
          'This will disconnect and remove the $_headerTitle from the app. You can pair it again from Settings.',
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
              'Disconnect',
              style: TextStyle(color: Colors.red),
            ),
          ),
        ],
      ),
    );

    if (confirmed != true || !mounted) return;

    AnalyticsService().logRhythmServerReset(wasOnline: _isOnline);
    setState(() => _isResetting = true);

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

  void _openDiagnostics() {
    AnalyticsService().logRhythmServerLogsOpened();
    _RhythmServerDiagnosticsScreen.show(context, client: _client);
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
                      ..._buildPerHubSections(http),
                      _buildHubPairingSuggestions(),
                      const SizedBox(height: 16),
                      _buildVersionSection(),
                      const SizedBox(height: 24),
                      if (_isEmbedded) ...[
                        _buildDiagnosticsButton(),
                        const SizedBox(height: 12),
                        _buildRebootButton(),
                        const SizedBox(height: 12),
                      ],
                      _buildResetButton(),
                      const SizedBox(height: 12),
                      _buildFactoryResetButton(),
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
              child: const Icon(
                Icons.close,
                color: _teal,
                size: 20,
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
    final connState = http.connectionState;
    final fullyOffline = !http.connected && !_isOnline;
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
                widget.hub.endpoint.host,
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

  Widget _buildVersionSection() {
    final syncProvider = context.watch<ServerSyncProvider>();
    final currentVersion = _otaService.currentVersion != '0.0.0'
        ? _otaService.currentVersion
        : syncProvider.firmwareVersion;
    final showOtaControls =
        _otaService.isLoadingSupport || _otaService.showUpdateUi;

    return _buildSection(
      title: _isEmbedded ? 'FIRMWARE' : 'VERSION',
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
                          : 'v$currentVersion',
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

              if (showOtaControls) ..._buildOtaStateContent(currentVersion),

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
        return [
          Divider(
            height: 1,
            color: CelestialColors.orbitRing.withValues(alpha: 0.3),
          ),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
            child: Row(
              mainAxisAlignment: MainAxisAlignment.spaceBetween,
              children: [
                Text(
                  'Available',
                  style: TextStyle(
                    color: CelestialColors.textSecondary.withValues(alpha: 0.8),
                    fontSize: 14,
                  ),
                ),
                Text(
                  'v${release.version}',
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
          if (release.changelog != null && release.changelog!.isNotEmpty)
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 0, 16, 8),
              child: Text(
                release.changelog!,
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.6),
                  fontSize: 13,
                ),
              ),
            ),
          _buildOtaButton(
            label: 'Install Update',
            icon: Icons.download_rounded,
            onTap: () {
              AnalyticsService().logOtaUpdateStarted(
                currentVersion,
                release.version,
              );
              _otaService.startUpdate(widget.hub.endpoint.host,
                  port: widget.hub.endpoint.port);
              _OtaUpdateOverlay.show(context, otaService: _otaService);
            },
          ),
        ];

      case OtaState.downloading:
      case OtaState.uploading:
        final pct = _otaService.progress;
        final label = _otaService.state == OtaState.downloading
            ? 'Downloading firmware...'
            : (_otaService.statusMessage ?? 'Installing update...');
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
              'Writing firmware to device...',
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
                  _otaService.statusMessage ?? 'Device restarting',
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
                  child: Text(
                    'Updated to v${_otaService.currentVersion}',
                    style: const TextStyle(
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
                    _otaService.errorMessage ?? 'Update failed',
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

  // ─── Per-Hub Sections ──────────────────────────────────────

  List<Widget> _buildPerHubSections(RhythmConnection http) {
    final syncProvider = context.watch<ServerSyncProvider>();
    final configuredHubs = syncProvider.serverHubInfos
        .where((h) => h['type'] != null && h['type'] != 'none')
        .toList();

    if (configuredHubs.isEmpty) return [];

    return [
      _buildSection(
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
                        color:
                            CelestialColors.orbitRing.withValues(alpha: 0.3)),
                  _buildHubRow(hub),
                ],
              ],
            ),
          ),
        ],
      ),
      const SizedBox(height: 16),
    ];
  }

  Widget _buildHubRow(Map<String, dynamic> hubInfo) {
    final type = hubInfo['type'] as String;
    final connected = hubInfo['connected'] as bool? ?? false;
    final label = _hubLabel(type);
    final hubColor = _hubColor(type);
    final deviceSummary = _hubSummaries[type] ??
        (_hubSummariesLoaded
            ? (connected ? 'Connected · no devices' : 'No devices')
            : 'Loading…');

    return GestureDetector(
      onTap: () => _HubDetailScreen.show(context, hubInfo: hubInfo),
      behavior: HitTestBehavior.opaque,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
        child: Row(
          children: [
            Icon(
              _hubIcon(type),
              color: hubColor.withValues(alpha: connected ? 1.0 : 0.5),
              size: 20,
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
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
                    deviceSummary,
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
                color: connected
                    ? const Color(0xFF22C55E)
                    : const Color(0xFFEF4444),
                boxShadow: !connected
                    ? [
                        BoxShadow(
                          color: const Color(0xFFEF4444).withValues(alpha: 0.6),
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

  Future<void> _startMatterAddFlow({
    MatterAddMethod? preferredMethod,
  }) async {
    await startMatterPairingFlow(
      context,
      preferredMethod: preferredMethod,
    );
    if (!mounted) return;
    await _fetchHubSummaries();
  }

  List<Widget> _buildMatterAddOptionRows(ServerSyncProvider syncProvider) {
    if (!syncProvider.canAddMatterDevice) return const [];
    return [
      _buildHubOptionRow(
        icon: Icons.memory_outlined,
        label: 'Add Matter Device',
        color: const Color(0xFF26A69A),
        onTap: () => _startMatterAddFlow(),
      ),
    ];
  }

  // ─── Hub Pairing Suggestions ─────────────────────────────

  Widget _buildHubPairingSuggestions() {
    final syncProvider = context.watch<ServerSyncProvider>();
    final configuredTypes = syncProvider.configuredHubTypes;
    final configuredHubs = syncProvider.serverHubInfos
        .where((h) => h['type'] != null && h['type'] != 'none')
        .toList();
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
      return const SizedBox.shrink();
    }

    // If no hubs configured at all, show as "LIGHT HUB" section
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
                      color: CelestialColors.orbitRing.withValues(alpha: 0.3)),
                  _buildHubOptionRow(
                    icon: Icons.home_outlined,
                    label: 'Home Assistant',
                    color: const Color(0xFF42A5F5),
                    isLoading: _isConfiguringHub,
                    onTap:
                        _isConfiguringHub ? null : () => _pairHa(syncProvider),
                  ),
                ],
                if (showHue) ...[
                  Divider(
                      height: 1,
                      color: CelestialColors.orbitRing.withValues(alpha: 0.3)),
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
                      color: CelestialColors.orbitRing.withValues(alpha: 0.3)),
                  ...matterOptions,
                ],
              ],
            ),
          ),
        ],
      );
    }

    // Hubs exist — show add options + disconnect all (if multiple)
    final children = <Widget>[];
    if (showHa || showHue || hasMatterOptions) {
      final options = <Widget>[];
      if (showHa) {
        options.add(_buildHubOptionRow(
          icon: Icons.home_outlined,
          label: 'Add Home Assistant',
          color: const Color(0xFF42A5F5),
          isLoading: _isConfiguringHub,
          onTap: _isConfiguringHub ? null : () => _pairHa(syncProvider),
        ));
      }
      if (showHue) {
        if (options.isNotEmpty) {
          options.add(Divider(
              height: 1,
              color: CelestialColors.orbitRing.withValues(alpha: 0.3)));
        }
        options.add(_buildHubOptionRow(
          icon: Icons.lightbulb_outline,
          label: 'Add Philips Hue',
          color: const Color(0xFFFFB900),
          onTap: () => HueConfiguratorScreen.show(context),
        ));
      }
      if (hasMatterOptions) {
        if (options.isNotEmpty) {
          options.add(Divider(
              height: 1,
              color: CelestialColors.orbitRing.withValues(alpha: 0.3)));
        }
        options.addAll(matterOptions);
      }
      children.add(
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
      );
    }

    if (children.isEmpty) return const SizedBox.shrink();

    return _buildSection(
      title: 'ADD HUB',
      children: children,
    );
  }

  Future<void> _pairHa(ServerSyncProvider syncProvider) async {
    if (_isHaAddon) {
      setState(() => _isConfiguringHub = true);
      try {
        await syncProvider.configureAddonHaHub();
      } finally {
        if (mounted) setState(() => _isConfiguringHub = false);
      }
    } else {
      final result = await HAConfiguratorScreen.show(context);
      if (result == true && mounted) {
        await syncProvider.pushHubCredentials(RoomSourceDto.homeAssistant);
      }
    }
  }

  Widget _buildHubOptionRow({
    required IconData icon,
    required String label,
    required Color color,
    bool isActive = false,
    bool isLoading = false,
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
              child: Text(
                label,
                style: TextStyle(
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

  Widget _buildDiagnosticsButton() {
    return GestureDetector(
      onTap: _isOnline ? _openDiagnostics : null,
      child: Container(
        padding: const EdgeInsets.symmetric(vertical: 14),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(14),
          color: _isOnline
              ? Colors.white.withValues(alpha: 0.04)
              : Colors.white.withValues(alpha: 0.02),
          border: Border.all(
            color: _isOnline
                ? CelestialColors.textSecondary.withValues(alpha: 0.2)
                : CelestialColors.textSecondary.withValues(alpha: 0.1),
          ),
        ),
        child: Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Icon(
              Icons.monitor_heart_outlined,
              color: _isOnline
                  ? CelestialColors.textSecondary
                  : CelestialColors.textSecondary.withValues(alpha: 0.4),
              size: 18,
            ),
            const SizedBox(width: 10),
            Text(
              'Diagnostics',
              style: TextStyle(
                color: _isOnline
                    ? CelestialColors.textSecondary
                    : CelestialColors.textSecondary.withValues(alpha: 0.4),
                fontSize: 15,
                fontWeight: FontWeight.w500,
              ),
            ),
          ],
        ),
      ),
    );
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

    final success = await _client.reboot();

    if (!mounted) return;

    setState(() => _isRebooting = false);

    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(
          success
              ? 'Rebooting $_headerTitle — it will reconnect shortly.'
              : 'Failed to send reboot command.',
        ),
        behavior: SnackBarBehavior.floating,
      ),
    );
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

  Widget _buildResetButton() {
    final buttonColor = Colors.red.shade400;

    return GestureDetector(
      onTap: _isResetting ? null : _handleReset,
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
            if (_isResetting)
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
              _isResetting ? 'Disconnecting...' : 'Disconnect $_headerTitle',
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

    final success = await _client.factoryReset();
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
    final disabled = _isFactoryResetting || _isResetting || _isRebooting;
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

class _DeviceCounts {
  int lights = 0;
  int buttons = 0;
  int motion = 0;
}

// =============================================================================
// OTA Update Overlay (full-screen blocking during firmware update)
// =============================================================================

class _OtaUpdateOverlay extends StatefulWidget {
  final OtaService otaService;

  const _OtaUpdateOverlay({required this.otaService});

  static Future<void> show(BuildContext context,
      {required OtaService otaService}) {
    return Navigator.of(context, rootNavigator: true).push(
      PageRouteBuilder(
        opaque: true,
        pageBuilder: (_, __, ___) => _OtaUpdateOverlay(otaService: otaService),
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
    with SingleTickerProviderStateMixin {
  late AnimationController _pulseController;
  late Animation<double> _pulseAnimation;

  static const _teal = Color(0xFF00BCD4);

  @override
  void initState() {
    super.initState();
    widget.otaService.addListener(_onStateChanged);

    _pulseController = AnimationController(
      duration: const Duration(milliseconds: 2000),
      vsync: this,
    )..repeat(reverse: true);

    _pulseAnimation = Tween<double>(begin: 0.3, end: 0.8).animate(
      CurvedAnimation(parent: _pulseController, curve: Curves.easeInOut),
    );
  }

  @override
  void dispose() {
    widget.otaService.removeListener(_onStateChanged);
    _pulseController.dispose();
    super.dispose();
  }

  void _onStateChanged() {
    if (mounted) setState(() {});
  }

  bool get _isFinished {
    final state = widget.otaService.state;
    return state == OtaState.complete || state == OtaState.error;
  }

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
      case OtaState.downloading:
      case OtaState.uploading:
        return _buildProgress();
      case OtaState.flashing:
        return _buildFlashing();
      case OtaState.rebooting:
        return _buildRebooting();
      case OtaState.complete:
        return _buildComplete();
      case OtaState.error:
        return _buildError();
      default:
        return _buildProgress();
    }
  }

  Widget _buildProgress() {
    final isDownloading = widget.otaService.state == OtaState.downloading;
    final pct = widget.otaService.progress;
    final message = isDownloading
        ? 'Downloading firmware...'
        : (widget.otaService.statusMessage ?? 'Installing on device...');

    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        AnimatedBuilder(
          animation: _pulseAnimation,
          builder: (context, _) {
            return Container(
              width: 80,
              height: 80,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: _teal.withValues(alpha: 0.15),
                boxShadow: [
                  BoxShadow(
                    color:
                        _teal.withValues(alpha: _pulseAnimation.value * 0.25),
                    blurRadius: 30,
                    spreadRadius: 4,
                  ),
                ],
              ),
              child: Icon(
                isDownloading
                    ? Icons.cloud_download_outlined
                    : Icons.system_update,
                color: _teal,
                size: 36,
              ),
            );
          },
        ),
        const SizedBox(height: 32),
        const Text(
          'Updating Firmware',
          style: TextStyle(
            color: CelestialColors.textPrimary,
            fontSize: 22,
            fontWeight: FontWeight.w600,
          ),
        ),
        const SizedBox(height: 8),
        Text(
          message,
          style: const TextStyle(
            color: CelestialColors.textSecondary,
            fontSize: 15,
          ),
        ),
        const SizedBox(height: 32),
        ClipRRect(
          borderRadius: BorderRadius.circular(4),
          child: LinearProgressIndicator(
            value: pct != null ? pct / 100.0 : null,
            backgroundColor: _teal.withValues(alpha: 0.1),
            valueColor: const AlwaysStoppedAnimation(_teal),
            minHeight: 8,
          ),
        ),
        if (pct != null) ...[
          const SizedBox(height: 12),
          Text(
            '$pct%',
            style: const TextStyle(
              color: _teal,
              fontSize: 20,
              fontWeight: FontWeight.w600,
              fontFamily: 'monospace',
            ),
          ),
        ],
        const SizedBox(height: 40),
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

  Widget _buildFlashing() {
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        AnimatedBuilder(
          animation: _pulseAnimation,
          builder: (context, _) {
            return Container(
              width: 80,
              height: 80,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: _teal.withValues(alpha: 0.15),
                boxShadow: [
                  BoxShadow(
                    color:
                        _teal.withValues(alpha: _pulseAnimation.value * 0.25),
                    blurRadius: 30,
                    spreadRadius: 4,
                  ),
                ],
              ),
              child: Center(
                child: SizedBox(
                  width: 32,
                  height: 32,
                  child: CircularProgressIndicator(
                    strokeWidth: 3,
                    valueColor: AlwaysStoppedAnimation(_teal),
                  ),
                ),
              ),
            );
          },
        ),
        const SizedBox(height: 32),
        const Text(
          'Updating Firmware',
          style: TextStyle(
            color: CelestialColors.textPrimary,
            fontSize: 22,
            fontWeight: FontWeight.w600,
          ),
        ),
        const SizedBox(height: 8),
        const Text(
          'Writing firmware to device...',
          style: TextStyle(
            color: CelestialColors.textSecondary,
            fontSize: 15,
          ),
        ),
        const SizedBox(height: 40),
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

  Widget _buildRebooting() {
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        AnimatedBuilder(
          animation: _pulseAnimation,
          builder: (context, _) {
            return Container(
              width: 80,
              height: 80,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: _teal.withValues(alpha: 0.15),
                boxShadow: [
                  BoxShadow(
                    color: _teal.withValues(alpha: _pulseAnimation.value * 0.2),
                    blurRadius: 30,
                    spreadRadius: 4,
                  ),
                ],
              ),
              child: Center(
                child: SizedBox(
                  width: 32,
                  height: 32,
                  child: CircularProgressIndicator(
                    strokeWidth: 3,
                    valueColor: AlwaysStoppedAnimation(_teal),
                  ),
                ),
              ),
            );
          },
        ),
        const SizedBox(height: 32),
        const Text(
          'Device restarting',
          style: TextStyle(
            color: CelestialColors.textPrimary,
            fontSize: 22,
            fontWeight: FontWeight.w600,
          ),
        ),
        const SizedBox(height: 8),
        Text(
          widget.otaService.statusMessage ??
              'Waiting for the device to come back online...',
          style: const TextStyle(
            color: CelestialColors.textSecondary,
            fontSize: 15,
          ),
          textAlign: TextAlign.center,
        ),
        const SizedBox(height: 40),
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

  Widget _buildComplete() {
    final version = widget.otaService.currentVersion;

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
          'Updated to v$version',
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
    final message = widget.otaService.errorMessage ?? 'Update failed';

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
          message,
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
// Diagnostics Screen (full-screen slide-up with Overview + Logs tabs)
// =============================================================================

class _RhythmServerDiagnosticsScreen extends StatefulWidget {
  final RhythmDiagnosticsApi client;

  const _RhythmServerDiagnosticsScreen({required this.client});

  static Future<void> show(BuildContext context,
      {required RhythmDiagnosticsApi client}) {
    return Navigator.of(context).push(
      PageRouteBuilder(
        opaque: false,
        barrierColor: Colors.black54,
        pageBuilder: (context, animation, secondaryAnimation) {
          return _RhythmServerDiagnosticsScreen(client: client);
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
  bool get _connected => widget.hubInfo['connected'] as bool? ?? false;
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
    context.watch<ServerSyncProvider>();
    final syncProvider = context.read<ServerSyncProvider>();
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
                    child: Text(
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
                            hubColor.withValues(
                                alpha: _connected ? 0.08 : 0.03),
                            CelestialColors.backgroundCard,
                          ],
                        ),
                        border: Border.all(
                          color: hubColor.withValues(
                              alpha: _connected ? 0.2 : 0.1),
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
                                  alpha: _connected ? 0.2 : 0.1),
                            ),
                            child: Icon(
                              hubIcon,
                              color: hubColor.withValues(
                                  alpha: _connected ? 1.0 : 0.5),
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
                                  color: _connected
                                      ? const Color(0xFF22C55E)
                                      : const Color(0xFFEF4444),
                                ),
                              ),
                              const SizedBox(width: 8),
                              Text(
                                _connected ? 'Connected' : 'Disconnected',
                                style: TextStyle(
                                  color: _connected
                                      ? const Color(0xFF22C55E)
                                      : const Color(0xFFEF4444),
                                  fontSize: 14,
                                  fontWeight: FontWeight.w500,
                                ),
                              ),
                            ],
                          ),
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
                        label: _connected ? 'Reconnect' : 'Retry',
                        color: _teal,
                        onTap: () => _retryHub(),
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
      onTap: () => DeviceDetailSheet.show(context, device, roomId),
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
    required VoidCallback onTap,
  }) {
    return GestureDetector(
      onTap: onTap,
      child: Container(
        padding: const EdgeInsets.symmetric(vertical: 14),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(14),
          color: color.withValues(alpha: 0.1),
          border: Border.all(
            color: color.withValues(alpha: 0.3),
          ),
        ),
        child: Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Icon(icon, color: color, size: 18),
            const SizedBox(width: 10),
            Text(
              label,
              style: TextStyle(
                color: color,
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

  Future<void> _retryHub() async {
    final syncProvider = context.read<ServerSyncProvider>();
    final source = switch (_type) {
      'hue' => RoomSourceDto.hue,
      'homeassistant' || 'home_assistant' => RoomSourceDto.homeAssistant,
      _ => null,
    };
    if (source != null) {
      await syncProvider.pushHubCredentials(source);
    } else {
      await syncProvider.connection.reconnect();
    }
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
