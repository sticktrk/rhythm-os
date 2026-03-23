import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import '../../widgets/solar_orbit.dart';
import '../../widgets/hub_status_indicator.dart';
import '../../widgets/success_modal.dart';
import '../../services/hue/hue_service_locator.dart';
import '../../services/hue_sse_storage.dart';
import '../../services/analytics_service.dart';
import '../../providers/server_sync_provider.dart';
import '../../providers/hub_connection_provider.dart';
import '../../providers/home_provider.dart';
import '../../providers/room_provider.dart';

/// Connection status for the Hue configurator.
enum HueLinkingStatus {
  notConfigured,
  discovering,
  bridgeFound,
  waitingForButton,
  linking,
  connected,
  error,
}

/// Full-screen hub management screen for Philips Hue.
///
/// When disconnected: shows discovery + push-link pairing flow.
/// When connected: shows status card with bridge IP + disconnect button.
///
/// Room discovery/management happens server-side — the app only handles
/// pairing (obtaining credentials) and pushing them to the server.
class HueConfiguratorScreen extends StatefulWidget {
  const HueConfiguratorScreen({super.key});

  /// Show the configurator as a full-screen modal.
  /// Returns true when connected, or null if cancelled.
  static Future<bool?> show(BuildContext context) {
    return Navigator.of(context).push<bool>(
      PageRouteBuilder(
        opaque: false,
        barrierColor: Colors.black54,
        pageBuilder: (context, animation, secondaryAnimation) {
          return const HueConfiguratorScreen();
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
  State<HueConfiguratorScreen> createState() => _HueConfiguratorScreenState();
}

class _HueConfiguratorScreenState extends State<HueConfiguratorScreen>
    with SingleTickerProviderStateMixin {
  HueLinkingStatus _status = HueLinkingStatus.notConfigured;
  String? _errorMessage;
  List<String> _discoveredBridges = [];
  String? _selectedBridgeIp;
  String? _connectedBridgeIp;
  bool _isFirstTimePairing = false;
  bool _isReconnecting = false;

  // Countdown for button press
  int _countdownSeconds = 30;
  Timer? _countdownTimer;
  Timer? _pairingPollTimer;

  late AnimationController _glowController;
  late Animation<double> _glowAnimation;

  @override
  void initState() {
    super.initState();
    AnalyticsService().logScreenView('hue_configurator');
    _loadSavedConfig();

    _glowController = AnimationController(
      duration: const Duration(milliseconds: 2000),
      vsync: this,
    )..repeat(reverse: true);

    _glowAnimation = Tween<double>(begin: 0.3, end: 0.6).animate(
      CurvedAnimation(parent: _glowController, curve: Curves.easeInOut),
    );
  }

  @override
  void dispose() {
    _countdownTimer?.cancel();
    _pairingPollTimer?.cancel();
    _glowController.dispose();
    super.dispose();
  }

  Future<void> _loadSavedConfig() async {
    final homeProvider = context.read<HomeProvider>();
    final hueHub = homeProvider.getFirstHubOfType(HubType.hue);

    if (hueHub != null && hueHub.hasCredentials) {
      if (!mounted) return;
      setState(() {
        _connectedBridgeIp = hueHub.endpoint.host;
        _status = HueLinkingStatus.connected;
      });
    }
  }

  // ─── Discovery & Pairing Logic ───────────────────────────

  Future<void> _startDiscovery() async {
    if (!mounted) return;
    setState(() {
      _status = HueLinkingStatus.discovering;
      _errorMessage = null;
      _discoveredBridges = [];
    });

    try {
      final bridges = await HueServiceLocator.instance.discoverBridges(
        timeout: const Duration(seconds: 8),
      );

      if (!mounted) return;

      if (bridges.isEmpty) {
        setState(() {
          _status = HueLinkingStatus.error;
          _errorMessage = 'No Hue bridges found on your network.';
        });
        AnalyticsService().logHubConnectionFailed('hue', 'No bridges found');
      } else if (bridges.length == 1) {
        setState(() {
          _discoveredBridges = bridges;
          _selectedBridgeIp = bridges.first;
          _status = HueLinkingStatus.bridgeFound;
        });
      } else {
        setState(() {
          _discoveredBridges = bridges;
          _status = HueLinkingStatus.bridgeFound;
        });
      }
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _status = HueLinkingStatus.error;
        _errorMessage = 'Discovery failed: ${e.toString()}';
      });
      AnalyticsService().logHubConnectionFailed('hue', 'Discovery failed: $e');
    }
  }

  void _startPairing() {
    if (_selectedBridgeIp == null) return;

    if (HueServiceLocator.isDemoMode) {
      _startDemoPairing();
      return;
    }

    setState(() {
      _status = HueLinkingStatus.waitingForButton;
      _countdownSeconds = 30;
    });

    _countdownTimer = Timer.periodic(const Duration(seconds: 1), (timer) {
      if (_countdownSeconds <= 0) {
        timer.cancel();
        _pairingPollTimer?.cancel();
        setState(() {
          _status = HueLinkingStatus.error;
          _errorMessage = 'Timed out waiting for button press.';
        });
        AnalyticsService().logHubConnectionFailed('hue', 'Pairing timeout');
      } else {
        setState(() {
          _countdownSeconds--;
        });
      }
    });

    _pairingPollTimer = Timer.periodic(const Duration(seconds: 2), (timer) async {
      final username = await HueServiceLocator.instance.pair(_selectedBridgeIp!);
      if (username != null) {
        timer.cancel();
        _countdownTimer?.cancel();
        await _onPairingSuccess(username);
      }
    });
  }

  Future<void> _startDemoPairing() async {
    final username = await HueServiceLocator.instance.pair(_selectedBridgeIp!);
    if (username != null) {
      await _onPairingSuccess(username);
    }
  }

  Future<void> _onPairingSuccess(String username) async {
    if (!mounted) return;
    setState(() {
      _status = HueLinkingStatus.linking;
    });

    final homeProvider = context.read<HomeProvider>();

    if (homeProvider.currentHome == null) {
      if (!mounted) return;
      setState(() {
        _status = HueLinkingStatus.error;
        _errorMessage = 'No home configured. Please complete setup first.';
      });
      return;
    }

    final existingHub = homeProvider.getFirstHubOfType(HubType.hue);

    Hub? savedHub;
    if (existingHub != null) {
      final updatedHub = existingHub.copyWith(
        endpoint: HubEndpoint(
          host: _selectedBridgeIp!,
          port: 443,
          useSsl: true,
        ),
        token: username,
        updatedAt: DateTime.now(),
        pendingSync: true,
      );
      final success = await homeProvider.updateHub(updatedHub);
      if (success) {
        savedHub = updatedHub;
      }
    } else {
      savedHub = await homeProvider.addHueHub(
        name: 'Philips Hue',
        bridgeIp: _selectedBridgeIp!,
        appKey: username,
      );
    }

    if (savedHub == null) {
      if (!mounted) return;
      setState(() {
        _status = HueLinkingStatus.error;
        _errorMessage = homeProvider.error ?? 'Failed to save hub configuration';
      });
      return;
    }

    HapticFeedback.heavyImpact();

    AnalyticsService().logHubConnected('hue');
    AnalyticsService().setHubType('hue');

    // Push credentials to server so it can connect to the hub
    if (mounted) {
      context.read<ServerSyncProvider>().pushHubCredentials(RoomSourceDto.hue);
    }

    if (!mounted) return;
    setState(() {
      _connectedBridgeIp = _selectedBridgeIp;
      _isFirstTimePairing = true;
      _status = HueLinkingStatus.connected;
    });

    if (_isFirstTimePairing && mounted) {
      await SuccessModal.show(
        context,
        roomCount: 0,
        hubType: 'Philips Hue',
      );
      if (mounted) {
        Navigator.of(context).popUntil((route) => route.isFirst);
      }
    }
  }

  // ─── Disconnect Logic ──────────────────────────────────────

  void _cancelPairing() {
    _countdownTimer?.cancel();
    _pairingPollTimer?.cancel();
    setState(() {
      _status = HueLinkingStatus.bridgeFound;
    });
  }

  Future<void> _disconnect() async {
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: const Text(
          'Disconnect',
          style: TextStyle(color: CelestialColors.textPrimary),
        ),
        content: const Text(
          'Are you sure you want to disconnect from this Hue bridge?',
          style: TextStyle(color: CelestialColors.textSecondary),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: Text(
              'Cancel',
              style: TextStyle(color: CelestialColors.textSecondary),
            ),
          ),
          TextButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text(
              'Disconnect',
              style: TextStyle(color: Colors.red),
            ),
          ),
        ],
      ),
    );

    if (confirmed == true) {
      AnalyticsService().logHubDisconnected('hue');
      AnalyticsService().setHubType(null);

      if (mounted) {
        // Tell the server to drop hub + rooms first (before local cleanup
        // triggers source-changed events that would re-push credentials).
        await context.read<ServerSyncProvider>().disconnectHub();
        context.read<HubConnectionProvider>().disconnect();
        await context.read<RoomProvider>().clearRoomsBySource(RoomSourceDto.hue);
      }

      final homeProvider = context.read<HomeProvider>();

      final hueHub = homeProvider.getFirstHubOfType(HubType.hue);
      if (hueHub != null) {
        await homeProvider.deleteHub(hueHub.id);
      }

      await HueSseStorage.clearAll();

      setState(() {
        _connectedBridgeIp = null;
        _status = HueLinkingStatus.notConfigured;
      });
    }
  }

  // ─── Build ───────────────────────────────────────────────

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
                    if (_status != HueLinkingStatus.connected)
                      _buildHeroSection(),
                    if (_status == HueLinkingStatus.connected)
                      _buildConnectedStatusCard(),
                    const SizedBox(height: 32),
                    _buildPairingContent(),
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
            onTap: () => Navigator.of(context).pop(
              _status == HueLinkingStatus.connected ? true : null,
            ),
            child: Container(
              width: 40,
              height: 40,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: const Color(0xFFFFB900).withValues(alpha: 0.15),
                border: Border.all(
                  color: const Color(0xFFFFB900).withValues(alpha: 0.3),
                  width: 1,
                ),
              ),
              child: const Icon(
                Icons.close,
                color: Color(0xFFFFB900),
                size: 20,
              ),
            ),
          ),
          const Expanded(
            child: Text(
              'Philips Hue',
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

  // ─── Connected State ─────────────────────────────────────

  Widget _buildConnectedStatusCard() {
    return Container(
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(
          color: const Color(0xFF22C55E).withValues(alpha: 0.2),
        ),
      ),
      child: Row(
        children: [
          // Bridge icon
          Container(
            width: 44,
            height: 44,
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(12),
              gradient: const LinearGradient(
                begin: Alignment.topLeft,
                end: Alignment.bottomRight,
                colors: [Color(0xFFFFB900), Color(0xFFFF8C00)],
              ),
            ),
            child: const Icon(
              Icons.lightbulb_outline,
              color: Colors.white,
              size: 22,
            ),
          ),
          const SizedBox(width: 14),
          // Connection info
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                const Text(
                  'Hue Bridge',
                  style: TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 15,
                    fontWeight: FontWeight.w600,
                  ),
                ),
                const SizedBox(height: 4),
                Text(
                  _connectedBridgeIp ?? 'Unknown',
                  style: TextStyle(
                    color: CelestialColors.textSecondary.withValues(alpha: 0.8),
                    fontSize: 13,
                    fontFamily: 'monospace',
                    letterSpacing: -0.3,
                  ),
                ),
              ],
            ),
          ),
          // Status indicator
          Consumer<HubConnectionProvider>(
            builder: (context, hubConnection, _) {
              return HubStatusIndicator(
                status: hubConnection.hueConnectionStatus,
                size: 10,
                showLabel: true,
              );
            },
          ),
        ],
      ),
    );
  }

  // ─── Pairing Flow (not configured) ───────────────────────

  Widget _buildHeroSection() {
    return AnimatedBuilder(
      animation: _glowAnimation,
      builder: (context, child) {
        final isWaiting = _status == HueLinkingStatus.waitingForButton;
        final glowIntensity = isWaiting ? 0.8 : _glowAnimation.value;

        return Container(
          padding: const EdgeInsets.all(24),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(24),
            gradient: RadialGradient(
              center: Alignment.center,
              radius: 1.2,
              colors: [
                const Color(0xFFFFB900).withValues(alpha: glowIntensity * 0.15),
                CelestialColors.backgroundCard,
              ],
            ),
            border: Border.all(
              color: const Color(0xFFFFB900).withValues(alpha: 0.2),
              width: 1,
            ),
          ),
          child: Column(
            children: [
              Stack(
                alignment: Alignment.center,
                children: [
                  if (isWaiting)
                    SizedBox(
                      width: 88,
                      height: 88,
                      child: CircularProgressIndicator(
                        value: _countdownSeconds / 30.0,
                        strokeWidth: 4,
                        backgroundColor: CelestialColors.orbitRing.withValues(alpha: 0.3),
                        valueColor: const AlwaysStoppedAnimation(Color(0xFFFFB900)),
                      ),
                    ),
                  Container(
                    width: 72,
                    height: 72,
                    decoration: BoxDecoration(
                      shape: BoxShape.circle,
                      gradient: const LinearGradient(
                        begin: Alignment.topLeft,
                        end: Alignment.bottomRight,
                        colors: [Color(0xFFFFB900), Color(0xFFFF8C00)],
                      ),
                      boxShadow: [
                        BoxShadow(
                          color: const Color(0xFFFFB900).withValues(alpha: glowIntensity),
                          blurRadius: isWaiting ? 32 : 24,
                          spreadRadius: isWaiting ? 4 : 2,
                        ),
                      ],
                    ),
                    child: const Icon(
                      Icons.lightbulb_outline,
                      color: Colors.white,
                      size: 36,
                    ),
                  ),
                ],
              ),
              const SizedBox(height: 16),
              Text(
                _getHeroTitle(),
                style: const TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 16,
                  fontWeight: FontWeight.w500,
                ),
              ),
              const SizedBox(height: 4),
              Text(
                _getHeroSubtitle(),
                textAlign: TextAlign.center,
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.8),
                  fontSize: 13,
                  height: 1.4,
                ),
              ),
              if (isWaiting) ...[
                const SizedBox(height: 12),
                Text(
                  '$_countdownSeconds seconds remaining',
                  style: TextStyle(
                    color: const Color(0xFFFFB900).withValues(alpha: 0.9),
                    fontSize: 14,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ],
            ],
          ),
        );
      },
    );
  }

  String _getHeroTitle() {
    switch (_status) {
      case HueLinkingStatus.notConfigured:
        return 'Connect your Hue Bridge';
      case HueLinkingStatus.discovering:
        return 'Searching for bridges...';
      case HueLinkingStatus.bridgeFound:
        return 'Bridge found';
      case HueLinkingStatus.waitingForButton:
        return 'Press the link button';
      case HueLinkingStatus.linking:
        return 'Linking...';
      case HueLinkingStatus.connected:
        return 'Connected';
      case HueLinkingStatus.error:
        return 'Connection failed';
    }
  }

  String _getHeroSubtitle() {
    switch (_status) {
      case HueLinkingStatus.notConfigured:
        return 'Control your Philips Hue lights with adaptive lighting';
      case HueLinkingStatus.discovering:
        return 'Looking for Hue bridges on your network';
      case HueLinkingStatus.bridgeFound:
        return _discoveredBridges.length > 1
            ? 'Found ${_discoveredBridges.length} bridges. Select one to continue.'
            : 'Found bridge at $_selectedBridgeIp';
      case HueLinkingStatus.waitingForButton:
        return 'Press the large button on top of your Hue bridge';
      case HueLinkingStatus.linking:
        return 'Completing connection...';
      case HueLinkingStatus.connected:
        return 'Connected to $_connectedBridgeIp';
      case HueLinkingStatus.error:
        return _errorMessage ?? 'An error occurred';
    }
  }

  Widget _buildPairingContent() {
    switch (_status) {
      case HueLinkingStatus.notConfigured:
        return _buildNotConfiguredContent();
      case HueLinkingStatus.discovering:
        return _buildDiscoveringContent();
      case HueLinkingStatus.bridgeFound:
        return _buildBridgeFoundContent();
      case HueLinkingStatus.waitingForButton:
        return _buildWaitingContent();
      case HueLinkingStatus.linking:
        return _buildProgressContent();
      case HueLinkingStatus.error:
        return _buildErrorContent();
      case HueLinkingStatus.connected:
        return _buildDisconnectButton();
    }
  }

  Widget _buildNotConfiguredContent() {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _buildActionButton(
          onTap: _startDiscovery,
          icon: Icons.wifi_find_rounded,
          label: 'Find Hue Bridge',
          isPrimary: true,
        ),
        const SizedBox(height: 24),
        _buildInfoCard(
          icon: Icons.info_outline_rounded,
          title: 'What is push-link pairing?',
          description:
              'For security, Hue bridges require you to press the physical button on the bridge to authorize new apps. This ensures only people with physical access can connect.',
        ),
      ],
    );
  }

  Widget _buildDiscoveringContent() {
    return Column(
      children: [
        const SizedBox(height: 24),
        const SizedBox(
          width: 48,
          height: 48,
          child: CircularProgressIndicator(
            strokeWidth: 3,
            valueColor: AlwaysStoppedAnimation(Color(0xFFFFB900)),
          ),
        ),
        const SizedBox(height: 16),
        Text(
          'Scanning your network...',
          style: TextStyle(
            color: CelestialColors.textSecondary,
            fontSize: 14,
          ),
        ),
      ],
    );
  }

  Widget _buildBridgeFoundContent() {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (_discoveredBridges.length > 1) ...[
          Text(
            'Select a bridge',
            style: TextStyle(
              color: CelestialColors.textSecondary,
              fontSize: 12,
              fontWeight: FontWeight.w600,
              letterSpacing: 0.5,
            ),
          ),
          const SizedBox(height: 12),
          ..._discoveredBridges.map((ip) => _buildBridgeOption(ip)),
          const SizedBox(height: 24),
        ],
        _buildActionButton(
          onTap: _selectedBridgeIp != null ? _startPairing : null,
          icon: Icons.link_rounded,
          label: 'Start Pairing',
          isPrimary: true,
        ),
        const SizedBox(height: 12),
        _buildActionButton(
          onTap: _startDiscovery,
          icon: Icons.refresh_rounded,
          label: 'Scan Again',
          isPrimary: false,
        ),
      ],
    );
  }

  Widget _buildBridgeOption(String ip) {
    final isSelected = _selectedBridgeIp == ip;
    return GestureDetector(
      onTap: () {
        setState(() {
          _selectedBridgeIp = ip;
        });
        HapticFeedback.selectionClick();
      },
      child: Container(
        margin: const EdgeInsets.only(bottom: 8),
        padding: const EdgeInsets.all(16),
        decoration: BoxDecoration(
          color: isSelected
              ? const Color(0xFFFFB900).withValues(alpha: 0.15)
              : CelestialColors.backgroundCard,
          borderRadius: BorderRadius.circular(14),
          border: Border.all(
            color: isSelected
                ? const Color(0xFFFFB900).withValues(alpha: 0.5)
                : CelestialColors.orbitRing.withValues(alpha: 0.5),
          ),
        ),
        child: Row(
          children: [
            Container(
              width: 44,
              height: 44,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: const Color(0xFFFFB900).withValues(alpha: 0.15),
              ),
              child: const Icon(
                Icons.router_rounded,
                color: Color(0xFFFFB900),
                size: 22,
              ),
            ),
            const SizedBox(width: 14),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  const Text(
                    'Hue Bridge',
                    style: TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 15,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    ip,
                    style: TextStyle(
                      color: CelestialColors.textSecondary.withValues(alpha: 0.8),
                      fontSize: 13,
                      fontFamily: 'monospace',
                    ),
                  ),
                ],
              ),
            ),
            if (isSelected)
              const Icon(
                Icons.check_circle_rounded,
                color: Color(0xFFFFB900),
                size: 24,
              )
            else
              Icon(
                Icons.radio_button_unchecked_rounded,
                color: CelestialColors.textSecondary.withValues(alpha: 0.5),
                size: 24,
              ),
          ],
        ),
      ),
    );
  }

  Widget _buildWaitingContent() {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _buildInfoCard(
          icon: Icons.touch_app_rounded,
          title: 'Press the button now',
          description:
              'Press the large circular button on top of your Hue bridge. The button has a small Philips Hue logo.',
        ),
        const SizedBox(height: 24),
        _buildActionButton(
          onTap: _cancelPairing,
          icon: Icons.close_rounded,
          label: 'Cancel',
          isPrimary: false,
        ),
      ],
    );
  }

  Widget _buildProgressContent() {
    return Column(
      children: [
        const SizedBox(height: 24),
        const SizedBox(
          width: 48,
          height: 48,
          child: CircularProgressIndicator(
            strokeWidth: 3,
            valueColor: AlwaysStoppedAnimation(Color(0xFFFFB900)),
          ),
        ),
        const SizedBox(height: 16),
        Text(
          'Completing connection...',
          style: TextStyle(
            color: CelestialColors.textSecondary,
            fontSize: 14,
          ),
        ),
      ],
    );
  }

  Widget _buildErrorContent() {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Container(
          padding: const EdgeInsets.all(16),
          decoration: BoxDecoration(
            color: Colors.red.shade400.withValues(alpha: 0.1),
            borderRadius: BorderRadius.circular(14),
            border: Border.all(
              color: Colors.red.shade400.withValues(alpha: 0.3),
            ),
          ),
          child: Row(
            children: [
              Icon(Icons.error_rounded, color: Colors.red.shade400, size: 20),
              const SizedBox(width: 12),
              Expanded(
                child: Text(
                  _errorMessage ?? 'Connection failed',
                  style: TextStyle(
                    color: Colors.red.shade400,
                    fontSize: 14,
                    fontWeight: FontWeight.w500,
                  ),
                ),
              ),
            ],
          ),
        ),
        const SizedBox(height: 24),
        _buildActionButton(
          onTap: _startDiscovery,
          icon: Icons.refresh_rounded,
          label: 'Try Again',
          isPrimary: true,
        ),
      ],
    );
  }

  Widget _buildDisconnectButton() {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _buildActionButton(
          onTap: _isReconnecting ? null : _reconnect,
          icon: Icons.refresh_rounded,
          label: _isReconnecting ? 'Reconnecting...' : 'Reconnect',
          isPrimary: true,
        ),
        const SizedBox(height: 12),
        _buildActionButton(
          onTap: _disconnect,
          icon: Icons.link_off_rounded,
          label: 'Disconnect',
          isPrimary: false,
          isDestructive: true,
        ),
      ],
    );
  }

  void _reconnect() {
    if (_isReconnecting) return;
    setState(() => _isReconnecting = true);
    context.read<ServerSyncProvider>().pushHubCredentials(RoomSourceDto.hue);
    // Clear after a short delay — SSE hub_status will update the real state.
    Future.delayed(const Duration(seconds: 3), () {
      if (mounted) setState(() => _isReconnecting = false);
    });
  }

  // ─── Shared UI Components ────────────────────────────────

  Widget _buildActionButton({
    required VoidCallback? onTap,
    required IconData icon,
    required String label,
    required bool isPrimary,
    bool isDestructive = false,
  }) {
    final isEnabled = onTap != null;
    final Color primaryColor =
        isDestructive ? Colors.red.shade400 : const Color(0xFFFFB900);

    return GestureDetector(
      onTap: onTap,
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 200),
        padding: const EdgeInsets.symmetric(vertical: 16),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(14),
          gradient: isPrimary && isEnabled
              ? LinearGradient(
                  begin: Alignment.topLeft,
                  end: Alignment.bottomRight,
                  colors: isDestructive
                      ? [Colors.red.shade400, Colors.red.shade600]
                      : [const Color(0xFFFFB900), const Color(0xFFFF8C00)],
                )
              : null,
          color: isPrimary
              ? (isEnabled ? null : CelestialColors.orbitRing.withValues(alpha: 0.3))
              : (isDestructive
                  ? Colors.red.shade400.withValues(alpha: 0.1)
                  : CelestialColors.backgroundCard),
          border: isPrimary
              ? null
              : Border.all(
                  color: isDestructive
                      ? Colors.red.shade400.withValues(alpha: 0.3)
                      : CelestialColors.orbitRing.withValues(alpha: 0.5),
                ),
          boxShadow: isPrimary && isEnabled
              ? [
                  BoxShadow(
                    color: primaryColor.withValues(alpha: 0.3),
                    blurRadius: 12,
                    offset: const Offset(0, 4),
                  ),
                ]
              : null,
        ),
        child: Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Icon(
              icon,
              color: isPrimary
                  ? (isEnabled
                      ? Colors.white
                      : CelestialColors.textSecondary.withValues(alpha: 0.5))
                  : (isDestructive ? Colors.red.shade400 : CelestialColors.textSecondary),
              size: 20,
            ),
            const SizedBox(width: 10),
            Text(
              label,
              style: TextStyle(
                color: isPrimary
                    ? (isEnabled
                        ? Colors.white
                        : CelestialColors.textSecondary.withValues(alpha: 0.5))
                    : (isDestructive ? Colors.red.shade400 : CelestialColors.textPrimary),
                fontSize: 15,
                fontWeight: FontWeight.w600,
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildInfoCard({
    required IconData icon,
    required String title,
    required String description,
  }) {
    return Container(
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(
          color: CelestialColors.orbitRing.withValues(alpha: 0.5),
        ),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Container(
            width: 36,
            height: 36,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: const Color(0xFFFFB900).withValues(alpha: 0.15),
            ),
            child: Icon(
              icon,
              color: const Color(0xFFFFB900),
              size: 18,
            ),
          ),
          const SizedBox(width: 14),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  title,
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 14,
                    fontWeight: FontWeight.w600,
                  ),
                ),
                const SizedBox(height: 4),
                Text(
                  description,
                  style: TextStyle(
                    color: CelestialColors.textSecondary.withValues(alpha: 0.8),
                    fontSize: 13,
                    height: 1.4,
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}
