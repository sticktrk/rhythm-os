import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' show RhythmConnection, RhythmConnectionState, RhythmDevice, RhythmDeviceType, RhythmDiagnosticsApi, RhythmRoom;
import '../../widgets/solar_orbit.dart';
import '../../providers/server_sync_provider.dart';
import '../../providers/home_provider.dart';
import '../../providers/room_provider.dart';
import '../../services/analytics_service.dart';
import '../../services/ota_service.dart';
import '../../widgets/device_detail_sheet.dart';
import 'ha_configurator_screen.dart';
import 'hue_configurator_screen.dart';
import 'matter_device_add_screen.dart';


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
  bool _isRefreshing = false;
  bool _isConfiguringHub = false;

  late AnimationController _glowController;
  late Animation<double> _glowAnimation;

  static const _teal = Color(0xFF00BCD4);
  static const _tealDeep = Color(0xFF0097A7);

  // ─── Server-type-aware computed getters ────────────────────

  String get _serverContext =>
      context.read<ServerSyncProvider>().serverPlatformContext;
  bool get _isEmbedded => _serverContext == 'embedded' || _serverContext == 'esp32';
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
    _client = RhythmDiagnosticsApi(host: widget.hub.endpoint.host, port: widget.hub.endpoint.port);

    _glowController = AnimationController(
      duration: const Duration(milliseconds: 2000),
      vsync: this,
    )..repeat(reverse: true);

    _glowAnimation = Tween<double>(begin: 0.3, end: 0.6).animate(
      CurvedAnimation(parent: _glowController, curve: Curves.easeInOut),
    );

    _otaService.addListener(_onOtaStateChanged);
    _checkHealth();

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

    if (_otaService.state == OtaState.complete) {
      AnalyticsService().logOtaUpdateCompleted(
        _otaService.availableRelease?.version ?? 'unknown',
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

  Future<void> _checkHealth() async {
    final online = await _client.healthCheck();
    if (mounted) {
      setState(() {
        _isOnline = online;
        _checkingHealth = false;
      });
    }
  }

  Future<void> _handleRefresh() async {
    if (_isRefreshing) return;
    setState(() {
      _isRefreshing = true;
      _checkingHealth = true;
    });

    await _checkHealth();

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
              child: SingleChildScrollView(
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
                    _buildDevicesSection(),
                    const SizedBox(height: 16),
                    _buildDeviceInfoSection(),
                    const SizedBox(height: 16),
                    _buildVersionSection(),
                    const SizedBox(height: 16),
                    _buildConnectionStatusSection(http),
                    const SizedBox(height: 24),
                    _buildCheckStatusButton(),
                    const SizedBox(height: 12),
                    if (_isEmbedded) ...[
                      _buildDiagnosticsButton(),
                      const SizedBox(height: 12),
                      _buildRebootButton(),
                      const SizedBox(height: 12),
                    ],
                    _buildResetButton(),
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
    final fullyOffline = !http.connected && !_isOnline;

    return AnimatedBuilder(
      animation: _glowAnimation,
      builder: (context, child) {
        // Dim static glow when fully offline, otherwise breathe
        final glowIntensity = fullyOffline ? 0.15 : _glowAnimation.value;

        return Container(
          padding: const EdgeInsets.all(24),
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
              const SizedBox(height: 16),
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
                  color: CelestialColors.textSecondary.withValues(alpha: 0.8),
                  fontSize: 13,
                  fontFamily: 'monospace',
                ),
              ),
            ],
          ),
        );
      },
    );
  }

  // ─── Sections ──────────────────────────────────────────────

  Widget _buildDeviceInfoSection() {
    return _buildSection(
      title: 'DEVICE INFO',
      children: [
        _buildInfoRow('IP Address', widget.hub.endpoint.host),
        if (!_isHaAddon)
          _buildInfoRow('mDNS', '${widget.hub.name}.local'),
      ],
    );
  }

  Widget _buildVersionSection() {
    final syncProvider = context.watch<ServerSyncProvider>();
    final currentVersion = syncProvider.firmwareVersion;

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
                      currentVersion == '0.0.0' ? 'Unknown' : 'v$currentVersion',
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

              // OTA controls (ESP32 only)
              if (_isEmbedded)
                ..._buildOtaStateContent(currentVersion),

              // HA addon managed note
              if (_isHaAddon)
                Padding(
                  padding: const EdgeInsets.fromLTRB(16, 0, 16, 12),
                  child: Row(
                    children: [
                      Icon(
                        Icons.info_outline_rounded,
                        color: CelestialColors.textSecondary.withValues(alpha: 0.5),
                        size: 16,
                      ),
                      const SizedBox(width: 8),
                      Text(
                        'Updates managed by Home Assistant',
                        style: TextStyle(
                          color: CelestialColors.textSecondary.withValues(alpha: 0.6),
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
                    color:
                        CelestialColors.textSecondary.withValues(alpha: 0.8),
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
              _otaService.startUpdate(widget.hub.endpoint.host, port: widget.hub.endpoint.port);
              _OtaUpdateOverlay.show(context, otaService: _otaService);
            },
          ),
        ];

      case OtaState.downloading:
      case OtaState.uploading:
        final pct = _otaService.progress;
        final label = _otaService.state == OtaState.downloading
            ? 'Downloading...'
            : 'Installing...';
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
                    color:
                        CelestialColors.textSecondary.withValues(alpha: 0.8),
                    fontSize: 14,
                  ),
                ),
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
                value: pct / 100.0,
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
                color:
                    CelestialColors.textSecondary.withValues(alpha: 0.8),
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
                  'Rebooting device...',
                  style: TextStyle(
                    color:
                        CelestialColors.textSecondary.withValues(alpha: 0.8),
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
                    'Updated to v${_otaService.availableRelease?.version ?? ""}',
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
                Icon(Icons.error_outline,
                    color: Colors.red.shade400, size: 18),
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

  Widget _buildConnectionStatusSection(RhythmConnection http) {
    return _buildSection(
      title: 'CONNECTION',
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
                label: 'Connection',
                statusText: _connectionStatusText(http.connectionState),
                statusColor: _connectionStatusColor(http.connectionState),
                isPulsing: http.connectionState ==
                        RhythmConnectionState.connecting ||
                    http.connectionState == RhythmConnectionState.reconnecting,
              ),
              Divider(
                height: 1,
                color: CelestialColors.orbitRing.withValues(alpha: 0.3),
              ),
              _buildConnectionRow(
                label: 'HTTP',
                statusText: _checkingHealth
                    ? 'Checking'
                    : (_isOnline ? 'Reachable' : 'Unreachable'),
                statusColor: _checkingHealth
                    ? CelestialColors.textSecondary
                    : (_isOnline
                        ? const Color(0xFF22C55E)
                        : Colors.red.shade400),
                isSpinning: _checkingHealth,
              ),
            ],
          ),
        ),
      ],
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
                    Divider(height: 1, color: CelestialColors.orbitRing.withValues(alpha: 0.3)),
                  _buildHubRow(hub, syncProvider),
                ],
              ],
            ),
          ),
        ],
      ),
      const SizedBox(height: 16),
    ];
  }

  Widget _buildHubRow(Map<String, dynamic> hubInfo, ServerSyncProvider syncProvider) {
    final type = hubInfo['type'] as String;
    final connected = hubInfo['connected'] as bool? ?? false;
    final label = _hubLabel(type);
    final hubColor = _hubColor(type);
    final deviceSummary = syncProvider.deviceSummaryForHub(type);

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
                      color: CelestialColors.textSecondary.withValues(alpha: 0.6),
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
                color: connected ? const Color(0xFF22C55E) : Colors.amber,
                boxShadow: !connected
                    ? [
                        BoxShadow(
                          color: Colors.amber.withValues(alpha: 0.6),
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
    _ => type,
  };

  static Color _hubColor(String type) => switch (type) {
    'hue' => const Color(0xFFFFB900),
    'homeassistant' || 'home_assistant' => const Color(0xFF42A5F5),
    _ => _teal,
  };

  static IconData _hubIcon(String type) => switch (type) {
    'hue' => Icons.lightbulb_outline,
    'homeassistant' || 'home_assistant' => Icons.home_outlined,
    _ => Icons.hub_outlined,
  };

  // ─── Hub Pairing Suggestions ─────────────────────────────

  Widget _buildHubPairingSuggestions() {
    final syncProvider = context.watch<ServerSyncProvider>();
    final configuredTypes = syncProvider.configuredHubTypes;
    final configuredHubs = syncProvider.serverHubInfos
        .where((h) => h['type'] != null && h['type'] != 'none')
        .toList();
    final hasAnyHub = configuredHubs.isNotEmpty;

    final showHa = !configuredTypes.contains('homeassistant');
    final showHue = !configuredTypes.contains('hue');

    if (!showHa && !showHue && !hasAnyHub) return const SizedBox.shrink();

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
                  Divider(height: 1, color: CelestialColors.orbitRing.withValues(alpha: 0.3)),
                  _buildHubOptionRow(
                    icon: Icons.home_outlined,
                    label: 'Home Assistant',
                    color: const Color(0xFF42A5F5),
                    isLoading: _isConfiguringHub,
                    onTap: _isConfiguringHub ? null : () => _pairHa(syncProvider),
                  ),
                ],
                if (showHue) ...[
                  Divider(height: 1, color: CelestialColors.orbitRing.withValues(alpha: 0.3)),
                  _buildHubOptionRow(
                    icon: Icons.lightbulb_outline,
                    label: 'Philips Hue',
                    color: const Color(0xFFFFB900),
                    onTap: () => HueConfiguratorScreen.show(context),
                  ),
                ],
              ],
            ),
          ),
        ],
      );
    }

    // Hubs exist — show add options + disconnect all (if multiple)
    final children = <Widget>[];
    if (showHa || showHue) {
      children.add(
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
              if (showHa)
                _buildHubOptionRow(
                  icon: Icons.home_outlined,
                  label: 'Add Home Assistant',
                  color: const Color(0xFF42A5F5),
                  isLoading: _isConfiguringHub,
                  onTap: _isConfiguringHub ? null : () => _pairHa(syncProvider),
                ),
              if (showHa && showHue)
                Divider(height: 1, color: CelestialColors.orbitRing.withValues(alpha: 0.3)),
              if (showHue)
                _buildHubOptionRow(
                  icon: Icons.lightbulb_outline,
                  label: 'Add Philips Hue',
                  color: const Color(0xFFFFB900),
                  onTap: () => HueConfiguratorScreen.show(context),
                ),
            ],
          ),
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
        syncProvider.pushHubCredentials(RoomSourceDto.homeAssistant);
      }
    }
  }

  // ─── Devices Section (Matter) ─────────────────────────────

  Widget _buildDevicesSection() {
    final syncProvider = context.watch<ServerSyncProvider>();
    final matterDevices = syncProvider.devicesForHub('matter');
    final matterRooms = syncProvider.roomsByHubType['matter'] ?? [];

    // Build device-id → room lookups.
    final deviceRoomName = <String, String>{};
    final deviceRoomId = <String, String>{};
    for (final room in matterRooms) {
      for (final did in room.deviceIds) {
        deviceRoomName[did] = room.name;
        deviceRoomId[did] = room.id;
      }
    }

    return _buildSection(
      title: matterDevices.length == 1 ? 'DEVICE' : 'DEVICES',
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
              for (final (index, device) in matterDevices.indexed) ...[
                if (index > 0)
                  Divider(
                      height: 1,
                      color:
                          CelestialColors.orbitRing.withValues(alpha: 0.3)),
                _buildMatterDeviceRow(
                    device, deviceRoomName[device.id], deviceRoomId[device.id]),
              ],
              if (matterDevices.isNotEmpty)
                Divider(
                    height: 1,
                    color: CelestialColors.orbitRing.withValues(alpha: 0.3)),
              _buildAddDeviceRow(matterDevices.isEmpty),
            ],
          ),
        ),
      ],
    );
  }

  Widget _buildMatterDeviceRow(
      RhythmDevice device, String? roomName, String? roomId) {
    return GestureDetector(
      onTap: () => DeviceDetailSheet.show(context, device, roomId ?? ''),
      behavior: HitTestBehavior.opaque,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
        child: Row(
          children: [
            Icon(
              Icons.lightbulb_outline,
              color: _teal.withValues(alpha: 0.8),
              size: 20,
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    device.displayName,
                    style: const TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 14,
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                  if (device.productInfo != null) ...[
                    const SizedBox(height: 2),
                    Text(
                      device.productInfo!,
                      style: TextStyle(
                        color: CelestialColors.textSecondary
                            .withValues(alpha: 0.6),
                        fontSize: 12,
                      ),
                    ),
                  ],
                ],
              ),
            ),
            if (roomName != null)
              Container(
                padding:
                    const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
                margin: const EdgeInsets.only(right: 8),
                decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(6),
                  color: _teal.withValues(alpha: 0.12),
                ),
                child: Text(
                  roomName,
                  style: TextStyle(
                    color: _teal.withValues(alpha: 0.9),
                    fontSize: 11,
                    fontWeight: FontWeight.w500,
                  ),
                ),
              )
            else
              Container(
                padding:
                    const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
                margin: const EdgeInsets.only(right: 8),
                decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(6),
                  color: Colors.amber.withValues(alpha: 0.12),
                ),
                child: Text(
                  'Unassigned',
                  style: TextStyle(
                    color: Colors.amber.withValues(alpha: 0.9),
                    fontSize: 11,
                    fontWeight: FontWeight.w500,
                  ),
                ),
              ),
            const Icon(
              Icons.chevron_right,
              color: CelestialColors.textSecondary,
              size: 18,
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildAddDeviceRow(bool isEmpty) {
    return GestureDetector(
      onTap: () async {
        await MatterDeviceAddScreen.show(context);
        if (mounted) {
          context.read<ServerSyncProvider>().connection.reconnect();
        }
      },
      behavior: HitTestBehavior.opaque,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
        child: Row(
          children: [
            Icon(
              Icons.add_circle_outline,
              color: _teal.withValues(alpha: 0.7),
              size: 20,
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  const Text(
                    'Add Device',
                    style: TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 14,
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                  if (isEmpty) ...[
                    const SizedBox(height: 2),
                    Text(
                      'Commission Matter lights directly',
                      style: TextStyle(
                        color: CelestialColors.textSecondary
                            .withValues(alpha: 0.6),
                        fontSize: 12,
                      ),
                    ),
                  ],
                ],
              ),
            ),
            const Icon(
              Icons.chevron_right,
              color: CelestialColors.textSecondary,
              size: 18,
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

  Widget _buildCheckStatusButton() {
    return GestureDetector(
      onTap: _isRefreshing ? null : _handleRefresh,
      child: Container(
        padding: const EdgeInsets.symmetric(vertical: 14),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(14),
          color: _teal.withValues(alpha: 0.1),
          border: Border.all(
            color: _teal.withValues(alpha: 0.3),
          ),
        ),
        child: Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            if (_isRefreshing)
              const SizedBox(
                width: 18,
                height: 18,
                child: CircularProgressIndicator(
                  strokeWidth: 2,
                  valueColor: AlwaysStoppedAnimation(_teal),
                ),
              )
            else
              const Icon(
                Icons.refresh_rounded,
                color: _teal,
                size: 20,
              ),
            const SizedBox(width: 10),
            Text(
              _isRefreshing ? 'Checking...' : 'Check Status',
              style: const TextStyle(
                color: _teal,
                fontSize: 15,
                fontWeight: FontWeight.w600,
              ),
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
              _isResetting
                  ? 'Disconnecting...'
                  : 'Disconnect $_headerTitle',
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

  Widget _buildInfoRow(String label, String value) {
    return Container(
      margin: const EdgeInsets.only(bottom: 1),
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(10),
      ),
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
          const SizedBox(width: 12),
          Flexible(
            child: Text(
              value,
              overflow: TextOverflow.ellipsis,
              textAlign: TextAlign.end,
              style: const TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 14,
                fontWeight: FontWeight.w500,
                fontFamily: 'monospace',
              ),
            ),
          ),
        ],
      ),
    );
  }
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
        pageBuilder: (_, __, ___) =>
            _OtaUpdateOverlay(otaService: otaService),
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
          isDownloading ? 'Downloading firmware...' : 'Installing on device...',
          style: const TextStyle(
            color: CelestialColors.textSecondary,
            fontSize: 15,
          ),
        ),
        const SizedBox(height: 32),
        ClipRRect(
          borderRadius: BorderRadius.circular(4),
          child: LinearProgressIndicator(
            value: pct / 100.0,
            backgroundColor: _teal.withValues(alpha: 0.1),
            valueColor: const AlwaysStoppedAnimation(_teal),
            minHeight: 8,
          ),
        ),
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
                    color:
                        _teal.withValues(alpha: _pulseAnimation.value * 0.2),
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
          'Rebooting Device',
          style: TextStyle(
            color: CelestialColors.textPrimary,
            fontSize: 22,
            fontWeight: FontWeight.w600,
          ),
        ),
        const SizedBox(height: 8),
        const Text(
          'Verifying update...',
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

  Widget _buildComplete() {
    final version = widget.otaService.availableRelease?.version ?? '';

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
                color:
                    CelestialColors.textSecondary.withValues(alpha: 0.2),
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
              child: _selectedTab == 0
                  ? _buildOverviewTab()
                  : _buildLogsTab(),
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
            color: isSelected ? _teal.withValues(alpha: 0.2) : Colors.transparent,
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

  static Future<void> show(BuildContext context, {required Map<String, dynamic> hubInfo}) {
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

  @override
  Widget build(BuildContext context) {
    final syncProvider = context.watch<ServerSyncProvider>();
    final http = context.read<RhythmConnection>();
    final rooms = syncProvider.roomsByHubType[_type] ?? [];
    final deviceSummary = syncProvider.deviceSummaryForHub(_type);
    final label = _RhythmServerSettingsScreenState._hubLabel(_type);
    final hubColor = _RhythmServerSettingsScreenState._hubColor(_type);
    final hubIcon = _RhythmServerSettingsScreenState._hubIcon(_type);

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
                            hubColor.withValues(alpha: _connected ? 0.08 : 0.03),
                            CelestialColors.backgroundCard,
                          ],
                        ),
                        border: Border.all(
                          color: hubColor.withValues(alpha: _connected ? 0.2 : 0.1),
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
                              color: hubColor.withValues(alpha: _connected ? 0.2 : 0.1),
                            ),
                            child: Icon(
                              hubIcon,
                              color: hubColor.withValues(alpha: _connected ? 1.0 : 0.5),
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
                                  color: _connected ? const Color(0xFF22C55E) : Colors.amber,
                                ),
                              ),
                              const SizedBox(width: 8),
                              Text(
                                _connected ? 'Connected' : 'Connecting...',
                                style: TextStyle(
                                  color: _connected ? const Color(0xFF22C55E) : Colors.amber,
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
                                color: CelestialColors.textSecondary.withValues(alpha: 0.7),
                                fontSize: 13,
                                fontFamily: 'monospace',
                              ),
                            ),
                          ],
                          const SizedBox(height: 4),
                          Text(
                            deviceSummary,
                            style: TextStyle(
                              color: CelestialColors.textSecondary.withValues(alpha: 0.6),
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
                    const SizedBox(height: 20),
                    // Debug section
                    _buildSectionHeader('DEBUG'),
                    const SizedBox(height: 8),
                    _buildDebugCard(http),
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
                    Divider(height: 1, color: CelestialColors.orbitRing.withValues(alpha: 0.2)),
                  // Room header
                  Padding(
                    padding: const EdgeInsets.only(left: 16, right: 16, top: 12, bottom: 4),
                    child: Row(
                      children: [
                        Icon(
                          Icons.meeting_room_outlined,
                          color: CelestialColors.textSecondary.withValues(alpha: 0.4),
                          size: 14,
                        ),
                        const SizedBox(width: 6),
                        Text(
                          room.name,
                          style: TextStyle(
                            color: CelestialColors.textSecondary.withValues(alpha: 0.6),
                            fontSize: 12,
                            fontWeight: FontWeight.w600,
                            letterSpacing: 0.3,
                          ),
                        ),
                        const Spacer(),
                        Text(
                          room.deviceSummary,
                          style: TextStyle(
                            color: CelestialColors.textSecondary.withValues(alpha: 0.4),
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
                          color: CelestialColors.textSecondary.withValues(alpha: 0.4),
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
                        color: CelestialColors.textSecondary.withValues(alpha: 0.5),
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

  // ─── Debug Card ──────────────────────────────────────────

  Widget _buildDebugCard(RhythmConnection http) {
    final sseEvents = http.lastSseEvents;
    final now = DateTime.now();

    return Container(
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(
          color: CelestialColors.orbitRing.withValues(alpha: 0.5),
        ),
      ),
      padding: const EdgeInsets.all(16),
      child: Column(
        children: [
          _buildDebugRow('SSE', http.sseConnected ? 'Connected' : 'Disconnected',
              color: http.sseConnected ? const Color(0xFF22C55E) : CelestialColors.textSecondary),
          if (!http.sseSupported)
            _buildDebugRow('SSE Supported', 'No',
                color: CelestialColors.textSecondary),
          _buildDebugRow('Last Activity', _relativeTime(http.lastSseActivity, now)),
          if (http.sseReconnectAttempts > 0)
            _buildDebugRow('Reconnects', '${http.sseReconnectAttempts}',
                color: Colors.amber),
          if (sseEvents.isNotEmpty) ...[
            Padding(
              padding: const EdgeInsets.only(top: 10, bottom: 4),
              child: Align(
                alignment: Alignment.centerLeft,
                child: Text(
                  'RECENT EVENTS',
                  style: TextStyle(
                    color: CelestialColors.textSecondary.withValues(alpha: 0.4),
                    fontSize: 10,
                    fontWeight: FontWeight.w600,
                    letterSpacing: 0.8,
                  ),
                ),
              ),
            ),
            for (final entry in sseEvents.entries)
              _buildDebugRow(
                entry.value.type,
                _relativeTime(entry.value.time, now),
              ),
          ],
        ],
      ),
    );
  }

  Widget _buildDebugRow(String label, String value, {Color? color}) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 3),
      child: Row(
        mainAxisAlignment: MainAxisAlignment.spaceBetween,
        children: [
          Text(
            label,
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.6),
              fontSize: 12,
              fontFamily: 'monospace',
            ),
          ),
          Text(
            value,
            style: TextStyle(
              color: color ?? CelestialColors.textPrimary.withValues(alpha: 0.8),
              fontSize: 12,
              fontFamily: 'monospace',
            ),
          ),
        ],
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
    RhythmDeviceType.light => (Icons.lightbulb_outline, const Color(0xFFFFB74D)),
    RhythmDeviceType.button => (Icons.touch_app_outlined, const Color(0xFF64B5F6)),
    RhythmDeviceType.motion => (Icons.sensors_outlined, const Color(0xFF81C784)),
  };

  String _relativeTime(DateTime time, DateTime now) {
    if (time.millisecondsSinceEpoch == 0) return 'Never';
    final diff = now.difference(time);
    if (diff.inSeconds < 2) return 'Just now';
    if (diff.inSeconds < 60) return '${diff.inSeconds}s ago';
    if (diff.inMinutes < 60) return '${diff.inMinutes}m ago';
    return '${diff.inHours}h ago';
  }

  void _retryHub() {
    final syncProvider = context.read<ServerSyncProvider>();
    final source = switch (_type) {
      'hue' => RoomSourceDto.hue,
      'homeassistant' || 'home_assistant' => RoomSourceDto.homeAssistant,
      _ => null,
    };
    if (source != null) {
      syncProvider.pushHubCredentials(source);
    }
  }

  void _disconnectHub() async {
    final syncProvider = context.read<ServerSyncProvider>();
    await syncProvider.disconnectOneHub(_type, _address ?? '');
    if (mounted) {
      syncProvider.connection.reconnect();
      Navigator.of(context).pop();
    }
  }
}
