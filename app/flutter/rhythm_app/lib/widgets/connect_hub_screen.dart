import 'dart:async';
import 'dart:io' show InternetAddress;
import 'package:bonsoir/bonsoir.dart';
import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/providers/hub_discovery.dart' show DiscoveredHub;
import 'package:rhythm_core/models/hub.dart' show HubType;
import 'solar_orbit.dart'; // For CelestialColors
import 'success_modal.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show RhythmConfigApi, RhythmDiagnosticsApi;
import '../providers/home_provider.dart';
import '../screens/hubs/ble_provisioning_screen.dart';
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

  const ConnectHubScreen(
      {super.key,
      this.mode = ConnectHubMode.rhythmServer,
      this.isModal = false});

  /// Show as a full-screen modal with slide-up transition and close button.
  static Future<void> show(BuildContext context,
      {ConnectHubMode mode = ConnectHubMode.rhythmServer}) {
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
                        color: CelestialColors.textSecondary
                            .withValues(alpha: 0.7),
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

  // Manual IP state (inline field)
  final _manualIpController = TextEditingController();
  final _manualIpFocus = FocusNode();
  bool _isManualConnecting = false;
  String? _manualConnectError;

  // RhythmServer branding
  static const _teal = Color(0xFF00BCD4);
  static const _tealDeep = Color(0xFF0097A7);

  // Hue branding
  static const _amber = Color(0xFFFFB900);
  static const _amberDeep = Color(0xFFFF8C00);

  Color get _primary => widget.mode == ConnectHubMode.hue ? _amber : _teal;
  Color get _primaryDeep =>
      widget.mode == ConnectHubMode.hue ? _amberDeep : _tealDeep;
  IconData get _icon => widget.mode == ConnectHubMode.hue
      ? Icons.lightbulb_outline
      : Icons.developer_board;

  String get _title => widget.mode == ConnectHubMode.hue
      ? 'Connect Philips Hue'
      : 'Find Your Rhythm Box';
  String get _subtitle => widget.mode == ConnectHubMode.hue
      ? 'Connect your Philips Hue bridge to get\nstarted with adaptive lighting'
      : 'We\'ll look for a Rhythm Box on\nyour local network';
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

    // Rebuild when focus state changes so the IP field border reflects it.
    _manualIpFocus.addListener(_onManualIpFocusChanged);

    // Auto-start mDNS scanning in rhythmServer mode
    if (widget.mode == ConnectHubMode.rhythmServer) {
      _scanForDevices();
    }
  }

  void _onManualIpFocusChanged() {
    if (mounted) setState(() {});
  }

  @override
  void dispose() {
    _bonsoirDiscovery?.stop();
    _manualIpFocus.removeListener(_onManualIpFocusChanged);
    _manualIpController.dispose();
    _manualIpFocus.dispose();
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
      final baseUrl = base.endsWith('/') ? base : '$base/';
      final devices = await RhythmConfigApi(baseUrl: baseUrl).discover();
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
        if (_isIgnorableBonsoirResolveError(e)) {
          debugPrint('mDNS: ignoring transient Bonsoir resolve error: $e');
          return;
        }
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

  bool _isIgnorableBonsoirResolveError(Object error) {
    return error is PlatformException &&
        error.code == 'discoveryError' &&
        error.message == 'discoveryServiceResolveFailed';
  }

  Future<void> _handleResolvedService(
    BonsoirService service,
    Set<String> seen,
    List<DiscoveredHub> found,
  ) async {
    final host = service.host ?? '';
    final name = service.name;
    final isRhythm =
        host.startsWith('rhythm-') || name.toLowerCase().contains('rhythm');
    if (!isRhythm) return;

    // Resolve mDNS hostname to IP address
    String ip;
    try {
      final hostname =
          host.endsWith('.') ? host.substring(0, host.length - 1) : host;
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
    final isHealthy =
        await RhythmDiagnosticsApi(host: ip, port: service.port).healthCheck();
    if (isHealthy && mounted) {
      found.add(hub);
      setState(() {
        _discoveredDevices = List.of(found);
      });
    }
  }

  /// Submit the inline manual-IP form.
  Future<void> _submitManualIp() async {
    final input = _manualIpController.text.trim();
    if (input.isEmpty) return;

    // Parse IP:port (default 54448)
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

    _manualIpFocus.unfocus();
    setState(() {
      _isManualConnecting = true;
      _manualConnectError = null;
    });

    final healthy =
        await RhythmDiagnosticsApi(host: ip, port: port).healthCheck();

    if (!mounted) return;
    if (!healthy) {
      setState(() {
        _isManualConnecting = false;
        _manualConnectError = 'Could not reach $ip:$port';
      });
      return;
    }

    setState(() => _isManualConnecting = false);
    final hub = DiscoveredHub(
      host: ip,
      port: port,
      address: ip,
      name: 'RhythmServer',
      type: HubType.server,
    );
    await _connectToDevice(hub);
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

  Future<void> _openBleProvisioning() async {
    AnalyticsService().logRhythmServerSetupTapped('ble');
    HapticFeedback.mediumImpact();
    await BleProvisioningScreen.show(context);
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: Listenable.merge([
        _breatheController,
        _rippleController,
      ]),
      builder: (context, _) {
        final isServer = widget.mode == ConnectHubMode.rhythmServer;
        final keyboardOpen = MediaQuery.of(context).viewInsets.bottom > 0;
        final bottomClearance = isServer ? 112.0 : 88.0;

        return Padding(
          padding: EdgeInsets.only(
            top: widget.isModal ? 24 : 8,
            bottom: bottomClearance,
          ),
          child: isServer
              ? _buildRhythmServerLayout(keyboardOpen)
              : _buildHueLayout(),
        );
      },
    );
  }

  // ── Layouts ────────────────────────────────────────────────────────────────

  Widget _buildRhythmServerLayout(bool keyboardOpen) {
    // Keep the IP TextField rendered in one stable spot so focus survives.
    // Only peripheral sections (hero, scan pill, new-box setup) collapse
    // away when the keyboard is up, giving the field room to lift.
    return Column(
      children: [
        if (!keyboardOpen) ...[
          _buildHeroSection(),
          const SizedBox(height: 8),
        ],
        _buildTitleAndStatus(),
        const SizedBox(height: 10),
        if (!keyboardOpen) ...[
          _buildScanButton(),
          const SizedBox(height: 10),
        ],
        Expanded(child: _buildMiddleContent()),
        const SizedBox(height: 10),
        _buildBottomActions(keyboardOpen),
        const SizedBox(height: 4),
      ],
    );
  }

  Widget _buildHueLayout() {
    return Column(
      children: [
        const SizedBox(height: 24),
        _buildHeroSection(),
        const SizedBox(height: 20),
        _buildTitleAndStatus(),
        const Spacer(),
        _buildConnectButton(),
        const SizedBox(height: 16),
      ],
    );
  }

  // ── Hero: compact beacon with breathing glow + ripples ─────────────────────

  Widget _buildHeroSection() {
    final b = _breathe.value;
    const heroSize = 150.0;
    const iconSize = 60.0;

    return SizedBox(
      width: heroSize,
      height: heroSize,
      child: Stack(
        alignment: Alignment.center,
        children: [
          // Ripple rings (3 staggered)
          for (int i = 0; i < 3; i++) _buildRippleRing(i, heroSize),

          // Ambient horizon glow
          Container(
            width: iconSize + 40 + b * 16,
            height: iconSize + 40 + b * 16,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              gradient: RadialGradient(
                colors: [
                  _primary.withValues(alpha: 0.10 + b * 0.06),
                  _primary.withValues(alpha: 0.02),
                  Colors.transparent,
                ],
                stops: const [0.0, 0.5, 1.0],
              ),
            ),
          ),

          // Central icon
          Container(
            width: iconSize,
            height: iconSize,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              gradient: LinearGradient(
                begin: Alignment.topLeft,
                end: Alignment.bottomRight,
                colors: [
                  _primary.withValues(alpha: 0.18 + b * 0.08),
                  _primaryDeep.withValues(alpha: 0.08 + b * 0.05),
                ],
              ),
              border: Border.all(
                color: _primary.withValues(alpha: 0.25 + b * 0.15),
                width: 1.2,
              ),
              boxShadow: [
                BoxShadow(
                  color: _primary.withValues(alpha: 0.12 + b * 0.12),
                  blurRadius: 24 + b * 12,
                  spreadRadius: b * 2,
                ),
              ],
            ),
            child: Icon(
              _icon,
              color: _primary.withValues(alpha: 0.78 + b * 0.22),
              size: 28,
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildRippleRing(int index, double size) {
    final phase = (_ripple.value + index * 0.33) % 1.0;
    final scale = 0.4 + phase * 0.6;
    final opacity = (1.0 - phase).clamp(0.0, 1.0) * 0.28;

    return Transform.scale(
      scale: scale,
      child: SizedBox(
        width: size,
        height: size,
        child: DecoratedBox(
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            border: Border.all(
              color: _primary.withValues(alpha: opacity),
              width: 1.0,
            ),
          ),
        ),
      ),
    );
  }

  // ── Title + Dynamic Status ─────────────────────────────────────────────────

  Widget _buildTitleAndStatus() {
    final isServer = widget.mode == ConnectHubMode.rhythmServer;

    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 32),
      child: Column(
        children: [
          Text(
            _title,
            textAlign: TextAlign.center,
            style: TextStyle(
              color: CelestialColors.textPrimary,
              fontSize: 22,
              fontWeight: FontWeight.w600,
              letterSpacing: 0.3,
              height: 1.1,
            ),
          ),
          const SizedBox(height: 8),
          if (isServer)
            _buildScanStatusLine()
          else
            Text(
              _subtitle,
              textAlign: TextAlign.center,
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.75),
                fontSize: 14,
                height: 1.45,
                letterSpacing: 0.1,
              ),
            ),
        ],
      ),
    );
  }

  Widget _buildScanStatusLine() {
    final count = _discoveredDevices.length;
    final b = _breathe.value;

    String label;
    if (_isScanning && count == 0) {
      label = 'Searching your network\u2026';
    } else if (count > 0) {
      label = count == 1 ? 'Found 1 box nearby' : 'Found $count boxes nearby';
    } else {
      label = 'No devices yet';
    }

    final isLive = _isScanning || count > 0;

    return Row(
      mainAxisAlignment: MainAxisAlignment.center,
      crossAxisAlignment: CrossAxisAlignment.center,
      children: [
        // Pulsing status dot (breathing teal while scanning, steady when found)
        Container(
          width: 7,
          height: 7,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: _isScanning
                ? _teal.withValues(alpha: 0.35 + b * 0.55)
                : (count > 0
                    ? _teal.withValues(alpha: 0.85)
                    : Colors.white.withValues(alpha: 0.22)),
            boxShadow: isLive
                ? [
                    BoxShadow(
                      color: _teal.withValues(alpha: 0.35 + b * 0.35),
                      blurRadius: 8 + b * 5,
                    ),
                  ]
                : null,
          ),
        ),
        const SizedBox(width: 9),
        Flexible(
          child: Text(
            label,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.85),
              fontSize: 13.5,
              letterSpacing: 0.2,
              fontWeight: FontWeight.w500,
            ),
          ),
        ),
      ],
    );
  }

  // ── Scan button (prominent primary action) ─────────────────────────────────

  Widget _buildScanButton() {
    final isScanning = _isScanning;

    return Center(
      child: GestureDetector(
        onTap: isScanning ? null : _scanForDevices,
        behavior: HitTestBehavior.opaque,
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 220),
          curve: Curves.easeOut,
          height: 40,
          padding: const EdgeInsets.symmetric(horizontal: 20),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(22),
            gradient: LinearGradient(
              begin: Alignment.topLeft,
              end: Alignment.bottomRight,
              colors: isScanning
                  ? [
                      _teal.withValues(alpha: 0.10),
                      _tealDeep.withValues(alpha: 0.06),
                    ]
                  : [
                      _teal.withValues(alpha: 0.28),
                      _tealDeep.withValues(alpha: 0.18),
                    ],
            ),
            border: Border.all(
              color: _teal.withValues(alpha: isScanning ? 0.3 : 0.55),
              width: 1.2,
            ),
            boxShadow: isScanning
                ? null
                : [
                    BoxShadow(
                      color: _teal.withValues(alpha: 0.22),
                      blurRadius: 16,
                      offset: const Offset(0, 4),
                      spreadRadius: -2,
                    ),
                  ],
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              if (isScanning)
                SizedBox(
                  width: 15,
                  height: 15,
                  child: CircularProgressIndicator(
                    strokeWidth: 1.8,
                    color: _teal.withValues(alpha: 0.85),
                  ),
                )
              else
                Icon(
                  Icons.radar_rounded,
                  color: _teal,
                  size: 17,
                ),
              const SizedBox(width: 9),
              Text(
                isScanning ? 'Scanning\u2026' : 'Scan Again',
                style: TextStyle(
                  color: isScanning
                      ? _teal.withValues(alpha: 0.8)
                      : CelestialColors.textPrimary,
                  fontSize: 14,
                  fontWeight: FontWeight.w600,
                  letterSpacing: 0.4,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }

  // ── Middle: devices list or empty state ────────────────────────────────────

  Widget _buildMiddleContent() {
    if (_discoveredDevices.isNotEmpty) {
      return _buildDiscoveredDevices();
    }

    // Empty state: soft guidance
    return Center(
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 40),
        child: Text(
          _isScanning
              ? 'Keep your Box powered on and\non the same Wi-Fi network'
              : 'Make sure your Box is powered on\nand connected to Wi-Fi',
          textAlign: TextAlign.center,
          style: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.45),
            fontSize: 12.5,
            height: 1.55,
            letterSpacing: 0.15,
          ),
        ),
      ),
    );
  }

  Widget _buildDiscoveredDevices() {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 24),
      child: ListView.separated(
        physics: const BouncingScrollPhysics(),
        padding: EdgeInsets.zero,
        itemCount: _discoveredDevices.length,
        separatorBuilder: (_, __) => const SizedBox(height: 8),
        itemBuilder: (context, index) =>
            _buildDeviceCard(_discoveredDevices[index]),
      ),
    );
  }

  Widget _buildDeviceCard(DiscoveredHub hub) {
    final hasError = _connectError == hub.address;
    final isBusy = _isConnecting && !hasError;
    final b = _breathe.value;

    return GestureDetector(
      onTap: _isConnecting ? null : () => _connectToDevice(hub),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(12),
          color: Colors.white.withValues(alpha: 0.04),
          border: Border.all(
            color: hasError
                ? Colors.red.withValues(alpha: 0.35)
                : _teal.withValues(alpha: 0.22),
            width: 1,
          ),
        ),
        child: Row(
          children: [
            // Icon puck with a tiny live pip in the top-right corner
            SizedBox(
              width: 34,
              height: 34,
              child: Stack(
                children: [
                  Container(
                    width: 34,
                    height: 34,
                    decoration: BoxDecoration(
                      shape: BoxShape.circle,
                      color: _teal.withValues(alpha: 0.12),
                    ),
                    child: Icon(
                      Icons.developer_board_rounded,
                      color: _teal.withValues(alpha: 0.9),
                      size: 17,
                    ),
                  ),
                  Positioned(
                    top: 1,
                    right: 1,
                    child: Container(
                      width: 7,
                      height: 7,
                      decoration: BoxDecoration(
                        shape: BoxShape.circle,
                        color: hasError
                            ? Colors.red.withValues(alpha: 0.85)
                            : Color.lerp(
                                _teal.withValues(alpha: 0.45),
                                _teal,
                                b,
                              ),
                        border: Border.all(
                          color:
                              CelestialColors.backgroundDark.withValues(alpha: 0.9),
                          width: 1.2,
                        ),
                      ),
                    ),
                  ),
                ],
              ),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisSize: MainAxisSize.min,
                children: [
                  Text(
                    hub.name ?? 'RhythmServer',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 14,
                      fontWeight: FontWeight.w500,
                      letterSpacing: 0.1,
                    ),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    hasError ? 'tap to retry' : hub.address,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: hasError
                          ? Colors.red.withValues(alpha: 0.75)
                          : CelestialColors.textSecondary
                              .withValues(alpha: 0.55),
                      fontSize: 11.5,
                      fontFamily: 'monospace',
                      letterSpacing: 0.3,
                    ),
                  ),
                ],
              ),
            ),
            if (isBusy)
              SizedBox(
                width: 16,
                height: 16,
                child: CircularProgressIndicator(
                  strokeWidth: 1.6,
                  color: _teal.withValues(alpha: 0.7),
                ),
              )
            else
              Icon(
                Icons.arrow_forward_rounded,
                color: _teal.withValues(alpha: 0.6),
                size: 18,
              ),
          ],
        ),
      ),
    );
  }

  // ── Bottom: compact secondary actions ──────────────────────────────────────

  Widget _buildBottomActions(bool keyboardOpen) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 24),
      child: Column(
        children: [
          // Manual IP sits with the "find an existing box" family
          _buildCompactManualIp(),
          if (!keyboardOpen && !kIsWeb) ...[
            const SizedBox(height: 14),
            _buildPairSectionDivider(),
            const SizedBox(height: 10),
            _buildCompactBleCard(),
          ],
        ],
      ),
    );
  }

  Widget _buildPairSectionDivider() {
    return Row(
      children: [
        Expanded(
          child: Container(
            height: 0.5,
            color: CelestialColors.textSecondary.withValues(alpha: 0.14),
          ),
        ),
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 12),
          child: Text(
            'FIRST TIME\u2009?',
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.42),
              fontSize: 9.5,
              fontWeight: FontWeight.w600,
              letterSpacing: 1.5,
            ),
          ),
        ),
        Expanded(
          child: Container(
            height: 0.5,
            color: CelestialColors.textSecondary.withValues(alpha: 0.14),
          ),
        ),
      ],
    );
  }

  Widget _buildCompactBleCard() {
    return GestureDetector(
      onTap: _openBleProvisioning,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(12),
          gradient: LinearGradient(
            begin: Alignment.topLeft,
            end: Alignment.bottomRight,
            colors: [
              _teal.withValues(alpha: 0.12),
              _tealDeep.withValues(alpha: 0.06),
            ],
          ),
          border: Border.all(color: _teal.withValues(alpha: 0.22)),
        ),
        child: Row(
          children: [
            Container(
              width: 32,
              height: 32,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: _teal.withValues(alpha: 0.15),
                border: Border.all(color: _teal.withValues(alpha: 0.22)),
              ),
              child: Icon(
                Icons.add_rounded,
                color: _teal.withValues(alpha: 0.92),
                size: 16,
              ),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisSize: MainAxisSize.min,
                children: [
                  Text(
                    'Set up a new Rhythm Box',
                    style: TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 13.5,
                      fontWeight: FontWeight.w600,
                      letterSpacing: 0.1,
                    ),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    'Guided setup in a few taps',
                    style: TextStyle(
                      color: CelestialColors.textSecondary
                          .withValues(alpha: 0.58),
                      fontSize: 11,
                      letterSpacing: 0.1,
                    ),
                  ),
                ],
              ),
            ),
            Icon(
              Icons.arrow_forward_rounded,
              color: _teal.withValues(alpha: 0.6),
              size: 18,
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildCompactManualIp() {
    final hasError = _manualConnectError != null;
    final isBusy = _isManualConnecting;

    return Container(
      height: 46,
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(12),
        color: Colors.white.withValues(alpha: 0.04),
        border: Border.all(
          color: hasError
              ? Colors.red.withValues(alpha: 0.4)
              : _manualIpFocus.hasFocus
                  ? _teal.withValues(alpha: 0.55)
                  : Colors.white.withValues(alpha: 0.10),
          width: 1,
        ),
      ),
      child: Row(
        children: [
          const SizedBox(width: 12),
          Icon(
            Icons.dns_rounded,
            color: _manualIpFocus.hasFocus
                ? _teal.withValues(alpha: 0.9)
                : CelestialColors.textSecondary.withValues(alpha: 0.55),
            size: 16,
          ),
          const SizedBox(width: 10),
          Expanded(
            child: TextField(
              controller: _manualIpController,
              focusNode: _manualIpFocus,
              enabled: !isBusy,
              keyboardType: TextInputType.url,
              textInputAction: TextInputAction.go,
              autocorrect: false,
              enableSuggestions: false,
              style: TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 14,
                fontFamily: 'monospace',
                letterSpacing: 0.4,
              ),
              decoration: InputDecoration(
                isDense: true,
                hintText: hasError
                    ? _manualConnectError
                    : 'Connect by IP (e.g. 192.168.1.100)',
                hintStyle: TextStyle(
                  color: hasError
                      ? Colors.red.withValues(alpha: 0.7)
                      : CelestialColors.textSecondary
                          .withValues(alpha: 0.42),
                  fontSize: 12.5,
                  fontFamily: hasError ? null : 'monospace',
                ),
                border: InputBorder.none,
                contentPadding: EdgeInsets.zero,
              ),
              onChanged: (_) {
                if (_manualConnectError != null) {
                  setState(() => _manualConnectError = null);
                }
              },
              onSubmitted: (_) => _submitManualIp(),
            ),
          ),
          GestureDetector(
            onTap: isBusy ? null : _submitManualIp,
            behavior: HitTestBehavior.opaque,
            child: Container(
              width: 44,
              height: 46,
              alignment: Alignment.center,
              child: isBusy
                  ? SizedBox(
                      width: 14,
                      height: 14,
                      child: CircularProgressIndicator(
                        strokeWidth: 1.6,
                        color: _teal.withValues(alpha: 0.75),
                      ),
                    )
                  : Icon(
                      Icons.arrow_forward_rounded,
                      color: _teal.withValues(alpha: 0.85),
                      size: 18,
                    ),
            ),
          ),
        ],
      ),
    );
  }

  // ── Hue-only primary button ────────────────────────────────────────────────

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

