import 'dart:async';
import 'dart:convert';
import 'dart:io'
    show
        HttpClient,
        HttpStatus,
        InternetAddress,
        InternetAddressType,
        NetworkInterface,
        Platform;
import 'package:bonsoir/bonsoir.dart';
import 'package:flutter/foundation.dart' show kIsWeb, visibleForTesting;
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

@visibleForTesting
const int rhythmServerDefaultPort = 54448;

const _mdnsProbeTimeout = Duration(milliseconds: 900);
const _mdnsProbeSettleTimeout = Duration(seconds: 2);
const _subnetProbeTimeout = Duration(milliseconds: 250);
const _subnetScanBatchSize = 32;

@visibleForTesting
List<String> rhythmDiscoveryHostCandidates(
  BonsoirService service, {
  bool includeGenericHttpServices = true,
}) {
  final hosts = <String>{};

  void addHost(String? value, {bool requireResolvableShape = false}) {
    final host = _normalizeMdnsHost(value);
    if (host == null) return;
    if (requireResolvableShape && !_hasResolvableHostShape(host)) return;
    hosts.add(host);
  }

  if (includeGenericHttpServices) {
    addHost(service.attributes['ip']);
    addHost(service.host);
    addHost(
      service.attributes['host'],
      requireResolvableShape: true,
    );
  } else if (rhythmMdnsServiceLooksLikeServer(service)) {
    addHost(service.host);
  }

  return List.unmodifiable(hosts);
}

@visibleForTesting
bool rhythmMdnsServiceLooksLikeServer(BonsoirService service) {
  final host = _normalizeMdnsHost(service.host) ?? '';
  final name = service.name.toLowerCase();
  return host.startsWith('rhythm-') || name.contains('rhythm');
}

@visibleForTesting
List<int> rhythmDiscoveryPortCandidates(
  BonsoirService service, {
  bool includeDefaultPort = true,
}) {
  final ports = <int>{};
  if (service.port > 0) ports.add(service.port);
  if (includeDefaultPort) ports.add(rhythmServerDefaultPort);
  return List.unmodifiable(ports);
}

@visibleForTesting
List<String> rhythmSubnetScanCandidates(Iterable<String> localAddresses) {
  final candidates = <String>{};
  for (final address in localAddresses) {
    final octets = _parseIpv4Octets(address);
    if (octets == null || !_isPrivateIpv4(octets)) continue;

    final prefix = '${octets[0]}.${octets[1]}.${octets[2]}.';
    for (var host = 1; host <= 254; host += 1) {
      if (host == octets[3]) continue;
      candidates.add('$prefix$host');
    }
  }
  return List.unmodifiable(candidates);
}

String? _normalizeMdnsHost(String? value) {
  final trimmed = value?.trim();
  if (trimmed == null || trimmed.isEmpty) return null;
  return trimmed.endsWith('.')
      ? trimmed.substring(0, trimmed.length - 1)
      : trimmed;
}

List<int>? _parseIpv4Octets(String value) {
  final parts = value.split('.');
  if (parts.length != 4) return null;

  final octets = <int>[];
  for (final part in parts) {
    final octet = int.tryParse(part);
    if (octet == null || octet < 0 || octet > 255) return null;
    octets.add(octet);
  }
  return octets;
}

bool _isPrivateIpv4(List<int> octets) {
  return octets[0] == 10 ||
      (octets[0] == 172 && octets[1] >= 16 && octets[1] <= 31) ||
      (octets[0] == 192 && octets[1] == 168);
}

bool _hasResolvableHostShape(String host) {
  return InternetAddress.tryParse(host) != null || host.contains('.');
}

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
    final pendingProbes = <Future<void>>{};
    try {
      final discovery = BonsoirDiscovery(type: '_http._tcp');
      _bonsoirDiscovery = discovery;
      await discovery.initialize();

      void queueProbe(BonsoirService service) {
        final probe = _handleResolvedService(service, seen, found)
            .catchError((Object e, StackTrace stackTrace) {
          debugPrint('mDNS: probe failed for ${service.name}: $e');
        });
        pendingProbes.add(probe);
        unawaited(probe.whenComplete(() => pendingProbes.remove(probe)));
      }

      discovery.eventStream?.listen((event) {
        switch (event) {
          case BonsoirDiscoveryServiceFoundEvent():
            event.service.resolve(discovery.serviceResolver).catchError((e) {
              debugPrint('mDNS: resolve failed for ${event.service.name}: $e');
            });
          case BonsoirDiscoveryServiceResolvedEvent():
            queueProbe(event.service);
          case BonsoirDiscoveryServiceUpdatedEvent():
            queueProbe(event.service);
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
      final probes = List<Future<void>>.of(pendingProbes);
      if (probes.isNotEmpty) {
        try {
          await Future.wait(probes).timeout(_mdnsProbeSettleTimeout);
        } on TimeoutException {
          debugPrint('mDNS: timed out waiting for health probes');
        }
      }
      if (Platform.isAndroid && found.isEmpty) {
        await _scanLocalSubnetForRhythmServers(found, seen);
      }
      _bonsoirDiscovery = null;
    } catch (e) {
      debugPrint('mDNS scan error: $e');
    }
  }

  bool _isIgnorableBonsoirResolveError(Object error) {
    return error is PlatformException &&
        error.code == 'discoveryError' &&
        (error.message == 'discoveryServiceResolveFailed' ||
            error.message == 'discoveryTxtResolveFailed');
  }

  Future<void> _handleResolvedService(
    BonsoirService service,
    Set<String> seen,
    List<DiscoveredHub> found,
  ) async {
    final hosts = rhythmDiscoveryHostCandidates(
      service,
      includeGenericHttpServices: Platform.isAndroid,
    );
    if (hosts.isEmpty) return;

    final ports = rhythmDiscoveryPortCandidates(
      service,
      includeDefaultPort: Platform.isAndroid,
    );
    for (final host in hosts) {
      final ip = await _resolveMdnsHost(host);
      if (ip == null) continue;

      for (final port in ports) {
        final key = '$ip:$port';
        if (seen.contains(key)) continue;
        seen.add(key);

        final isHealthy = await _checkDiscoveredRhythmServer(ip, port);

        if (isHealthy && mounted) {
          final hub = DiscoveredHub(
            host: host,
            port: port,
            address: ip,
            name: _friendlyMdnsServiceName(service),
            type: HubType.server,
          );
          found.add(hub);
          setState(() {
            _discoveredDevices = List.of(found);
          });
          return;
        }
      }
    }
  }

  Future<void> _scanLocalSubnetForRhythmServers(
    List<DiscoveredHub> found,
    Set<String> seen,
  ) async {
    final localAddresses = await _localIpv4Addresses();
    final candidates = rhythmSubnetScanCandidates(localAddresses);
    if (candidates.isEmpty) return;

    debugPrint('mDNS: scanning local subnet for Rhythm servers');
    for (var index = 0;
        index < candidates.length && mounted;
        index += _subnetScanBatchSize) {
      final batch = candidates.skip(index).take(_subnetScanBatchSize).toList();
      final results = await Future.wait(
        batch.map((ip) async {
          final key = '$ip:$rhythmServerDefaultPort';
          if (seen.contains(key)) return null;
          seen.add(key);

          final isHealthy = await _isRhythmServerHealthy(
            host: ip,
            port: rhythmServerDefaultPort,
            timeout: _subnetProbeTimeout,
          );
          return isHealthy ? ip : null;
        }),
      );

      final healthyIps = results.whereType<String>().toList();
      if (healthyIps.isEmpty) continue;
      for (final ip in healthyIps) {
        found.add(
          DiscoveredHub(
            host: ip,
            port: rhythmServerDefaultPort,
            address: ip,
            name: 'RhythmServer',
            type: HubType.server,
          ),
        );
      }
      setState(() {
        _discoveredDevices = List.of(found);
      });
      return;
    }
  }

  Future<bool> _checkDiscoveredRhythmServer(String host, int port) {
    if (Platform.isAndroid) {
      return _isRhythmServerHealthy(
        host: host,
        port: port,
        timeout: _mdnsProbeTimeout,
      );
    }
    return RhythmDiagnosticsApi(host: host, port: port).healthCheck();
  }

  Future<List<String>> _localIpv4Addresses() async {
    try {
      final interfaces = await NetworkInterface.list(
        includeLoopback: false,
        type: InternetAddressType.IPv4,
      );
      return [
        for (final interface in interfaces)
          for (final address in interface.addresses) address.address,
      ];
    } catch (e) {
      debugPrint('mDNS: could not list local network interfaces: $e');
      return const [];
    }
  }

  Future<bool> _isRhythmServerHealthy({
    required String host,
    required int port,
    required Duration timeout,
  }) async {
    final client = HttpClient()..connectionTimeout = timeout;
    try {
      final request = await client
          .getUrl(Uri.parse('http://$host:$port/health'))
          .timeout(timeout);
      final response = await request.close().timeout(timeout);
      if (response.statusCode != HttpStatus.ok) {
        await response.drain();
        return false;
      }

      final body = await utf8.decoder.bind(response).join().timeout(timeout);
      final payload = jsonDecode(body);
      return payload is Map && payload['status'] == 'healthy';
    } catch (_) {
      return false;
    } finally {
      client.close(force: true);
    }
  }

  Future<String?> _resolveMdnsHost(String host) async {
    final literalAddress = InternetAddress.tryParse(host);
    if (literalAddress != null) return literalAddress.address;

    try {
      final addresses = await InternetAddress.lookup(host);
      for (final address in addresses) {
        if (address.type == InternetAddressType.IPv4) {
          return address.address;
        }
      }
      if (addresses.isNotEmpty) return addresses.first.address;
    } catch (_) {
      debugPrint('mDNS: Could not resolve $host to IP');
    }
    return null;
  }

  String _friendlyMdnsServiceName(BonsoirService service) {
    final attrHost = service.attributes['host']?.trim();
    if (attrHost != null && attrHost.isNotEmpty) return attrHost;
    return service.name.isNotEmpty ? service.name : 'RhythmServer';
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
      port = int.tryParse(parts[1]) ?? rhythmServerDefaultPort;
    } else {
      ip = input;
      port = rhythmServerDefaultPort;
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
                          color: CelestialColors.backgroundDark
                              .withValues(alpha: 0.9),
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
                      color:
                          CelestialColors.textSecondary.withValues(alpha: 0.58),
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
                      : CelestialColors.textSecondary.withValues(alpha: 0.42),
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
