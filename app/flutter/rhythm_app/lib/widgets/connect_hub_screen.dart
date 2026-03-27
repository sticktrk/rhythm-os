import 'dart:async';
import 'dart:io' show InternetAddress;
import 'package:bonsoir/bonsoir.dart';
import 'package:dio/dio.dart';
import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/providers/hub_discovery.dart' show DiscoveredHub;
import 'package:rhythm_core/models/hub.dart' show HubType;
import 'solar_orbit.dart'; // For CelestialColors
import 'success_modal.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' show RhythmDiagnosticsApi;
import '../providers/home_provider.dart';
import '../screens/hubs/hue_configurator_screen.dart';
import '../services/analytics_service.dart';

/// Which empty-state variant to show.
enum ConnectHubMode { rhythmServer, hue }

/// Full-screen empty state shown when setup is incomplete.
///
/// [ConnectHubMode.rhythmServer] — teal, developer_board icon, opens ESP32 provisioning.
/// [ConnectHubMode.hue] — amber, lightbulb icon, opens Hue configurator.
class ConnectHubScreen extends StatefulWidget {
  final ConnectHubMode mode;

  /// Whether this screen is shown as a modal (with close button) vs inline.
  final bool isModal;

  const ConnectHubScreen({super.key, this.mode = ConnectHubMode.rhythmServer, this.isModal = false});

  /// Show as a full-screen modal with slide-up transition and close button.
  static Future<void> show(BuildContext context, {ConnectHubMode mode = ConnectHubMode.rhythmServer}) {
    return Navigator.of(context).push(
      PageRouteBuilder(
        opaque: false,
        barrierColor: Colors.black54,
        pageBuilder: (context, animation, secondaryAnimation) {
          return Scaffold(
            backgroundColor: CelestialColors.backgroundDark,
            body: SafeArea(
              child: Stack(
                children: [
                  ConnectHubScreen(mode: mode, isModal: true),
                  Positioned(
                    top: 8,
                    right: 8,
                    child: IconButton(
                      onPressed: () => Navigator.of(context).pop(),
                      icon: Icon(
                        Icons.close,
                        color: CelestialColors.textSecondary.withValues(alpha: 0.7),
                        size: 24,
                      ),
                    ),
                  ),
                ],
              ),
            ),
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
  State<ConnectHubScreen> createState() => _ConnectHubScreenState();
}

class _ConnectHubScreenState extends State<ConnectHubScreen>
    with TickerProviderStateMixin {
  late AnimationController _breatheController;
  late Animation<double> _breathe;

  late AnimationController _rippleController;
  late Animation<double> _ripple;

  // mDNS discovery state (rhythmServer mode only)
  List<DiscoveredHub> _discoveredDevices = [];
  bool _isScanning = false;
  bool _isConnecting = false;
  String? _connectError;
  BonsoirDiscovery? _bonsoirDiscovery;

  // Manual IP input
  final _manualIpController = TextEditingController();
  bool _isManualConnecting = false;
  String? _manualConnectError;

  // RhythmServer branding
  static const _teal = Color(0xFF00BCD4);
  static const _tealDeep = Color(0xFF0097A7);

  // Hue branding
  static const _amber = Color(0xFFFFB900);
  static const _amberDeep = Color(0xFFFF8C00);

  Color get _primary => widget.mode == ConnectHubMode.hue ? _amber : _teal;
  Color get _primaryDeep => widget.mode == ConnectHubMode.hue ? _amberDeep : _tealDeep;
  IconData get _icon => widget.mode == ConnectHubMode.hue ? Icons.lightbulb_outline : Icons.developer_board;

  String get _title => widget.mode == ConnectHubMode.hue ? 'Connect Philips Hue' : 'Pair RhythmServer';
  String get _subtitle => widget.mode == ConnectHubMode.hue
      ? 'Connect your Philips Hue bridge to get\nstarted with adaptive lighting'
      : 'Connect your RhythmServer to get\nstarted with adaptive lighting';
  String get _buttonLabel {
    return 'Connect Philips Hue';
  }

  @override
  void initState() {
    super.initState();

    // Slow breathing glow on the central icon
    _breatheController = AnimationController(
      duration: const Duration(milliseconds: 4000),
      vsync: this,
    )..repeat(reverse: true);
    _breathe = CurvedAnimation(
      parent: _breatheController,
      curve: Curves.easeInOut,
    );

    // Concentric ripple rings expanding outward
    _rippleController = AnimationController(
      duration: const Duration(milliseconds: 4500),
      vsync: this,
    )..repeat();
    _ripple = CurvedAnimation(
      parent: _rippleController,
      curve: Curves.easeOut,
    );

    AnalyticsService().logScreenView('connect_hub');

    // Auto-start mDNS scanning in rhythmServer mode
    if (widget.mode == ConnectHubMode.rhythmServer) {
      _scanForDevices();
    }
  }

  @override
  void dispose() {
    _bonsoirDiscovery?.stop();
    _manualIpController.dispose();
    _breatheController.dispose();
    _rippleController.dispose();
    super.dispose();
  }

  Future<void> _scanForDevices() async {
    setState(() {
      _isScanning = true;
      _discoveredDevices = [];
    });

    final found = <DiscoveredHub>[];

    if (kIsWeb) {
      await _scanViaWebApi(found);
    } else {
      await _scanViaBonsoir(found);
    }

    if (mounted) {
      AnalyticsService().logMdnsScanCompleted(found.length);
      setState(() {
        _isScanning = false;
      });
    }
  }

  /// Web: call GET api/discover on the same origin (rhythm-server / addon ingress).
  Future<void> _scanViaWebApi(List<DiscoveredHub> found) async {
    try {
      final base = Uri.base.toString();
      final dio = Dio(BaseOptions(
        baseUrl: base.endsWith('/') ? base : '$base/',
        connectTimeout: const Duration(seconds: 5),
        receiveTimeout: const Duration(seconds: 5),
      ));
      final response = await dio.get('api/discover');
      final List<dynamic> devices = response.data is List ? response.data : [];
      for (final device in devices) {
        final hub = DiscoveredHub(
          host: device['host'] as String? ?? '',
          port: device['port'] as int? ?? 80,
          address: device['address'] as String? ?? '',
          name: device['name'] as String? ?? '',
          type: HubType.server,
        );
        if (hub.address.isNotEmpty) {
          found.add(hub);
          if (mounted) {
            setState(() {
              _discoveredDevices = List.of(found);
            });
          }
        }
      }
    } catch (e) {
      debugPrint('Web discover error: $e');
    }
  }

  /// Native: use Bonsoir (mDNS) to discover devices.
  Future<void> _scanViaBonsoir(List<DiscoveredHub> found) async {
    final seen = <String>{};
    try {
      final discovery = BonsoirDiscovery(type: '_http._tcp');
      _bonsoirDiscovery = discovery;
      await discovery.initialize();

      discovery.eventStream?.listen((event) {
        switch (event) {
          case BonsoirDiscoveryServiceFoundEvent():
            event.service.resolve(discovery.serviceResolver).catchError((e) {
              debugPrint('mDNS: resolve failed for ${event.service.name}: $e');
            });
          case BonsoirDiscoveryServiceResolvedEvent():
            _handleResolvedService(event.service, seen, found);
          default:
            break;
        }
      }, onError: (e) {
        debugPrint('mDNS: discovery stream error: $e');
      });

      await discovery.start();
      await Future.delayed(const Duration(seconds: 5));
      await discovery.stop();
      _bonsoirDiscovery = null;
    } catch (e) {
      debugPrint('mDNS scan error: $e');
    }
  }

  Future<void> _handleResolvedService(
    BonsoirService service,
    Set<String> seen,
    List<DiscoveredHub> found,
  ) async {
    final host = service.host ?? '';
    final name = service.name;
    final isRhythm = host.startsWith('rhythm-') ||
        name.toLowerCase().contains('rhythm');
    if (!isRhythm) return;

    // Resolve mDNS hostname to IP address
    String ip;
    try {
      final hostname = host.endsWith('.') ? host.substring(0, host.length - 1) : host;
      final addresses = await InternetAddress.lookup(hostname);
      ip = addresses.first.address;
    } catch (_) {
      debugPrint('mDNS: Could not resolve $host to IP');
      return;
    }

    final key = '$ip:${service.port}';
    if (seen.contains(key)) return;
    seen.add(key);

    final hub = DiscoveredHub(
      host: host,
      port: service.port,
      address: ip,
      name: name,
      type: HubType.server,
    );

    // Verify device is actually reachable before showing it
    final isHealthy = await RhythmDiagnosticsApi(host: ip, port: service.port).healthCheck();
    if (isHealthy && mounted) {
      found.add(hub);
      setState(() {
        _discoveredDevices = List.of(found);
      });
    }
  }

  /// Connect to a manually entered IP address.
  Future<void> _connectManualIp() async {
    final input = _manualIpController.text.trim();
    if (input.isEmpty) return;

    // Parse IP:port (default port 54448 for rhythm-server)
    String ip;
    int port;
    if (input.contains(':')) {
      final parts = input.split(':');
      ip = parts[0];
      port = int.tryParse(parts[1]) ?? 54448;
    } else {
      ip = input;
      port = 54448;
    }

    setState(() {
      _isManualConnecting = true;
      _manualConnectError = null;
    });

    final client = RhythmDiagnosticsApi(host: ip, port: port);
    final isHealthy = await client.healthCheck();

    if (!mounted) return;

    if (isHealthy) {
      final hub = DiscoveredHub(
        host: ip,
        port: port,
        address: ip,
        name: 'RhythmServer',
        type: HubType.server,
      );
      await _connectToDevice(hub);
    } else {
      setState(() {
        _isManualConnecting = false;
        _manualConnectError = 'Could not connect to $ip:$port';
      });
    }
  }

  Future<void> _connectToDevice(DiscoveredHub hub) async {
    AnalyticsService().logRhythmServerDiscoveredConnect(hub.address);
    if (!kIsWeb) HapticFeedback.mediumImpact();
    setState(() {
      _isConnecting = true;
      _connectError = null;
    });

    final client = RhythmDiagnosticsApi(host: hub.address, port: hub.port);
    final isHealthy = await client.healthCheck();

    if (!mounted) return;

    if (isHealthy) {
      final result = await context.read<HomeProvider>().addServerHub(
        name: hub.name ?? 'RhythmServer',
        host: hub.address,
        port: hub.port,
      );

      if (!mounted) return;

      if (result == null) {
        setState(() {
          _isConnecting = false;
          _isManualConnecting = false;
          _connectError = hub.address;
        });
        return;
      }

      if (!kIsWeb) HapticFeedback.heavyImpact();
      await SuccessModal.show(
        context,
        roomCount: 0,
        hubType: 'RhythmServer',
      );

      if (mounted && widget.isModal) {
        Navigator.of(context).pop();
      }
    } else {
      setState(() {
        _isConnecting = false;
        _isManualConnecting = false;
        _connectError = hub.address;
      });
    }
  }

  void _openConfigurator() {
    AnalyticsService().logRhythmServerSetupTapped(
      widget.mode == ConnectHubMode.hue ? 'hue' : 'rhythmServer',
    );
    HapticFeedback.mediumImpact();
    if (widget.mode == ConnectHubMode.hue) {
      HueConfiguratorScreen.show(context);
    }
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: Listenable.merge([
        _breatheController,
        _rippleController,
      ]),
      builder: (context, _) {
        return Column(
          children: [
            const Spacer(flex: 2),
            // Ripple rings + central icon
            _buildHeroSection(),
            const SizedBox(height: 48),
            // Text content
            _buildTextContent(),
            // Scanning indicator or discovered devices
            if (widget.mode == ConnectHubMode.rhythmServer) ...[
              if (_isScanning && _discoveredDevices.isEmpty)
                _buildScanningIndicator(),
              if (_discoveredDevices.isNotEmpty)
                _buildDiscoveredDevices(),
              if (!_isScanning)
                _buildScanAgainButton(),
              const SizedBox(height: 16),
              _buildManualIpInput(),
            ],
            const SizedBox(height: 36),
            // Connect button (Hue mode only)
            if (widget.mode == ConnectHubMode.hue)
              _buildConnectButton(),
            const Spacer(flex: 3),
          ],
        );
      },
    );
  }

  Widget _buildHeroSection() {
    final b = _breathe.value;
    const iconSize = 80.0;

    return SizedBox(
      width: 260,
      height: 260,
      child: Stack(
        alignment: Alignment.center,
        children: [
          // Ripple rings (3 staggered)
          for (int i = 0; i < 3; i++)
            _buildRippleRing(i),

          // Ambient horizon glow behind the icon
          Container(
            width: iconSize + 60 + b * 20,
            height: iconSize + 60 + b * 20,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              gradient: RadialGradient(
                colors: [
                  _primary.withValues(alpha: 0.08 + b * 0.06),
                  _primary.withValues(alpha: 0.02),
                  Colors.transparent,
                ],
                stops: const [0.0, 0.5, 1.0],
              ),
            ),
          ),

          // Central icon container
          Container(
            width: iconSize,
            height: iconSize,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              gradient: LinearGradient(
                begin: Alignment.topLeft,
                end: Alignment.bottomRight,
                colors: [
                  _primary.withValues(alpha: 0.15 + b * 0.08),
                  _primaryDeep.withValues(alpha: 0.08 + b * 0.05),
                ],
              ),
              border: Border.all(
                color: _primary.withValues(alpha: 0.2 + b * 0.15),
                width: 1.5,
              ),
              boxShadow: [
                BoxShadow(
                  color: _primary.withValues(alpha: 0.1 + b * 0.12),
                  blurRadius: 32 + b * 16,
                  spreadRadius: b * 4,
                ),
              ],
            ),
            child: Icon(
              _icon,
              color: _primary.withValues(alpha: 0.7 + b * 0.3),
              size: 36,
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildRippleRing(int index) {
    // Each ring is offset in phase
    final phase = (_ripple.value + index * 0.33) % 1.0;
    // Rings expand from 0.3 to 1.0 of container size and fade out
    final scale = 0.35 + phase * 0.65;
    final opacity = (1.0 - phase).clamp(0.0, 1.0) * 0.25;

    return Transform.scale(
      scale: scale,
      child: Container(
        decoration: BoxDecoration(
          shape: BoxShape.circle,
          border: Border.all(
            color: _primary.withValues(alpha: opacity),
            width: 1.0,
          ),
        ),
      ),
    );
  }

  Widget _buildTextContent() {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 40),
      child: Column(
        children: [
          Text(
            _title,
            style: TextStyle(
              color: CelestialColors.textPrimary,
              fontSize: 24,
              fontWeight: FontWeight.w600,
              letterSpacing: 0.5,
            ),
          ),
          const SizedBox(height: 12),
          Text(
            _subtitle,
            textAlign: TextAlign.center,
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.85),
              fontSize: 15,
              height: 1.5,
              letterSpacing: 0.2,
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildScanningIndicator() {
    return Padding(
      padding: const EdgeInsets.only(top: 16),
      child: Row(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          SizedBox(
            width: 12,
            height: 12,
            child: CircularProgressIndicator(
              strokeWidth: 1.5,
              color: CelestialColors.textSecondary.withValues(alpha: 0.5),
            ),
          ),
          const SizedBox(width: 8),
          Text(
            'Searching your network\u2026',
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.6),
              fontSize: 13,
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildDiscoveredDevices() {
    return Padding(
      padding: const EdgeInsets.only(top: 24, left: 32, right: 32),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            'Found on your network',
            textAlign: TextAlign.center,
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.6),
              fontSize: 12,
              fontWeight: FontWeight.w500,
              letterSpacing: 0.5,
            ),
          ),
          const SizedBox(height: 10),
          for (final hub in _discoveredDevices)
            _buildDeviceCard(hub),
        ],
      ),
    );
  }

  Widget _buildScanAgainButton() {
    return Padding(
      padding: const EdgeInsets.only(top: 12),
      child: GestureDetector(
        onTap: _scanForDevices,
        child: Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Icon(
              Icons.refresh,
              color: _teal.withValues(alpha: 0.7),
              size: 15,
            ),
            const SizedBox(width: 6),
            Text(
              'Scan again',
              style: TextStyle(
                color: _teal.withValues(alpha: 0.7),
                fontSize: 13,
                fontWeight: FontWeight.w500,
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildDeviceCard(DiscoveredHub hub) {
    final hasError = _connectError == hub.address;
    final isBusy = _isConnecting && !hasError;

    return Padding(
      padding: const EdgeInsets.only(bottom: 8),
      child: GestureDetector(
        onTap: _isConnecting ? null : () => _connectToDevice(hub),
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(14),
            color: Colors.white.withValues(alpha: 0.06),
            border: Border.all(
              color: hasError
                  ? Colors.red.withValues(alpha: 0.3)
                  : _teal.withValues(alpha: 0.15),
              width: 1,
            ),
          ),
          child: Row(
            children: [
              Container(
                width: 36,
                height: 36,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: _teal.withValues(alpha: 0.12),
                ),
                child: Icon(
                  Icons.developer_board,
                  color: _teal.withValues(alpha: 0.8),
                  size: 18,
                ),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      hub.name ?? 'RhythmServer',
                      style: TextStyle(
                        color: CelestialColors.textPrimary,
                        fontSize: 15,
                        fontWeight: FontWeight.w500,
                      ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      hasError ? 'Connection failed - tap to retry' : hub.address,
                      style: TextStyle(
                        color: hasError
                            ? Colors.red.withValues(alpha: 0.7)
                            : CelestialColors.textSecondary.withValues(alpha: 0.6),
                        fontSize: 12,
                      ),
                    ),
                  ],
                ),
              ),
              if (isBusy)
                SizedBox(
                  width: 18,
                  height: 18,
                  child: CircularProgressIndicator(
                    strokeWidth: 2,
                    color: _teal.withValues(alpha: 0.6),
                  ),
                )
              else
                Icon(
                  Icons.chevron_right,
                  color: CelestialColors.textSecondary.withValues(alpha: 0.4),
                  size: 20,
                ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _buildManualIpInput() {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 32),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            'Or enter IP address',
            textAlign: TextAlign.center,
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.6),
              fontSize: 12,
              fontWeight: FontWeight.w500,
              letterSpacing: 0.5,
            ),
          ),
          const SizedBox(height: 8),
          Row(
            children: [
              Expanded(
                child: Container(
                  height: 44,
                  decoration: BoxDecoration(
                    borderRadius: BorderRadius.circular(12),
                    color: Colors.white.withValues(alpha: 0.06),
                    border: Border.all(
                      color: _manualConnectError != null
                          ? Colors.red.withValues(alpha: 0.3)
                          : _teal.withValues(alpha: 0.15),
                      width: 1,
                    ),
                  ),
                  child: TextField(
                    controller: _manualIpController,
                    style: TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 14,
                    ),
                    decoration: InputDecoration(
                      hintText: '192.168.1.100:54448',
                      hintStyle: TextStyle(
                        color: CelestialColors.textSecondary.withValues(alpha: 0.4),
                        fontSize: 14,
                      ),
                      border: InputBorder.none,
                      contentPadding: const EdgeInsets.symmetric(horizontal: 14),
                    ),
                    onSubmitted: (_) => _connectManualIp(),
                  ),
                ),
              ),
              const SizedBox(width: 8),
              GestureDetector(
                onTap: _isManualConnecting ? null : _connectManualIp,
                child: Container(
                  width: 44,
                  height: 44,
                  decoration: BoxDecoration(
                    borderRadius: BorderRadius.circular(12),
                    color: _teal.withValues(alpha: 0.15),
                    border: Border.all(
                      color: _teal.withValues(alpha: 0.2),
                      width: 1,
                    ),
                  ),
                  child: _isManualConnecting
                      ? Center(
                          child: SizedBox(
                            width: 18,
                            height: 18,
                            child: CircularProgressIndicator(
                              strokeWidth: 2,
                              color: _teal.withValues(alpha: 0.6),
                            ),
                          ),
                        )
                      : Icon(
                          Icons.arrow_forward,
                          color: _teal.withValues(alpha: 0.8),
                          size: 20,
                        ),
                ),
              ),
            ],
          ),
          if (_manualConnectError != null)
            Padding(
              padding: const EdgeInsets.only(top: 6),
              child: Text(
                _manualConnectError!,
                textAlign: TextAlign.center,
                style: TextStyle(
                  color: Colors.red.withValues(alpha: 0.7),
                  fontSize: 12,
                ),
              ),
            ),
        ],
      ),
    );
  }

  Widget _buildConnectButton() {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 48),
      child: GestureDetector(
        onTap: _openConfigurator,
        child: Container(
          padding: const EdgeInsets.symmetric(vertical: 16),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(16),
            gradient: LinearGradient(
              begin: Alignment.topLeft,
              end: Alignment.bottomRight,
              colors: [_primary, _primaryDeep],
            ),
            boxShadow: [
              BoxShadow(
                color: _primary.withValues(alpha: 0.35),
                blurRadius: 20,
                offset: const Offset(0, 6),
                spreadRadius: -2,
              ),
              BoxShadow(
                color: _primaryDeep.withValues(alpha: 0.2),
                blurRadius: 40,
                offset: const Offset(0, 12),
                spreadRadius: -4,
              ),
            ],
          ),
          child: Row(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              Icon(
                _icon,
                color: Colors.white,
                size: 20,
              ),
              const SizedBox(width: 10),
              Text(
                _buttonLabel,
                style: const TextStyle(
                  color: Colors.white,
                  fontSize: 16,
                  fontWeight: FontWeight.w600,
                  letterSpacing: 0.3,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
