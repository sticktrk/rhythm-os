import 'dart:async';
import 'dart:io' show Platform;
import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:permission_handler/permission_handler.dart';
import '../../widgets/solar_orbit.dart';
import '../../widgets/success_modal.dart';
import 'package:provider/provider.dart';
import '../../providers/home_provider.dart';
import '../../services/analytics_service.dart';
import '../../services/ble_provisioning_service.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' show RhythmConnection, RhythmConnectionState;
import '../../services/rhythm_accessory_service.dart';

/// State machine for the ESP32 provisioning flow.
enum Esp32ProvisioningStatus {
  ready,
  scanning,
  deviceFound,
  connecting,
  wifiSetup,
  provisioning,
  success,
  error,
}

/// Full-screen ESP32 BLE provisioning screen.
///
/// Walks the user through:
/// 1. Scanning for nearby RhythmBox devices via BLE
/// 2. Connecting to a discovered device
/// 3. Entering WiFi credentials
/// 4. Provisioning the device onto the network
///
class Esp32ProvisioningScreen extends StatefulWidget {
  const Esp32ProvisioningScreen({super.key});

  /// Show the provisioning screen as a full-screen modal.
  static Future<void> show(BuildContext context) {
    return Navigator.of(context).push(
      PageRouteBuilder(
        opaque: false,
        barrierColor: Colors.black54,
        pageBuilder: (context, animation, secondaryAnimation) {
          return const Esp32ProvisioningScreen();
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
  State<Esp32ProvisioningScreen> createState() =>
      _Esp32ProvisioningScreenState();
}

class _Esp32ProvisioningScreenState extends State<Esp32ProvisioningScreen>
    with SingleTickerProviderStateMixin {
  Esp32ProvisioningStatus _status = Esp32ProvisioningStatus.ready;
  String? _errorMessage;
  String? _wifiErrorMessage;

  final _bleService = BleProvisioningService();
  final _accessoryService = RhythmAccessoryService();
  StreamSubscription<BleDevice>? _scanSubscription;

  bool get _isIOS => !kIsWeb && Platform.isIOS;

  /// Use AccessorySetupKit only on iOS 18+; older iOS falls back to manual BLE scan.
  bool get _useASK {
    if (!_isIOS) return false;
    try {
      // Platform.operatingSystemVersion on iOS returns e.g. "17.5.1 (21F90)"
      final version = Platform.operatingSystemVersion;
      final match = RegExp(r'^(\d+)\.').firstMatch(version);
      if (match != null) {
        return int.parse(match.group(1)!) >= 18;
      }
    } catch (_) {}
    return true; // default to ASK — safe fallback for modern iOS
  }

  List<BleDevice> _discoveredDevices = [];
  BleDevice? _selectedDevice;
  BleDeviceInfo? _deviceInfo;
  String? _provisionedIp;

  final _ssidController = TextEditingController();
  final _passwordController = TextEditingController();
  bool _obscurePassword = true;

  late AnimationController _glowController;
  late Animation<double> _glowAnimation;

  static const _teal = Color(0xFF00BCD4);
  static const _tealDeep = Color(0xFF0097A7);

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

    _ssidController.addListener(() {
      if (mounted) setState(() {});
    });

    AnalyticsService().logScreenView('esp32_provisioning');
  }

  @override
  void dispose() {
    _scanSubscription?.cancel();
    _bleService.dispose();
    _ssidController.dispose();
    _passwordController.dispose();
    _glowController.dispose();
    super.dispose();
  }

  // ─── Bluetooth Permissions ─────────────────────────────────

  Future<bool> _ensureBluetoothPermission() async {
    // Web doesn't need native permissions
    if (kIsWeb) return true;

    // iOS uses Permission.bluetooth (triggers the Core Bluetooth system dialog).
    // Android 12+ uses bluetoothScan + bluetoothConnect; older Android needs location.
    final permissions = <Permission>[];
    if (Platform.isIOS) {
      permissions.add(Permission.bluetooth);
    } else if (Platform.isAndroid) {
      permissions.addAll([
        Permission.bluetoothScan,
        Permission.bluetoothConnect,
        Permission.locationWhenInUse,
      ]);
    }

    var statuses = await permissions.request();

    var denied = statuses.entries
        .where((e) => !e.value.isGranted)
        .toList();

    // iOS race condition: CBManagerAuthorization may not have propagated yet
    // when permission_handler checks the status right after the system dialog
    // dismisses. Re-check after a brief delay.
    if (denied.isNotEmpty && !kIsWeb && Platform.isIOS) {
      await Future<void>.delayed(const Duration(milliseconds: 600));
      final rechecked = <Permission, PermissionStatus>{};
      for (final p in permissions) {
        rechecked[p] = await p.status;
      }
      denied = rechecked.entries
          .where((e) => !e.value.isGranted)
          .toList();
      statuses = rechecked;
    }

    if (denied.isEmpty) return true;

    // Check if any were permanently denied
    final permanentlyDenied = denied.any((e) => e.value.isPermanentlyDenied);

    if (!mounted) return false;

    if (permanentlyDenied) {
      _showPermissionDeniedDialog();
    } else {
      setState(() {
        _status = Esp32ProvisioningStatus.error;
        _errorMessage = 'Bluetooth permission is required to find nearby controllers.';
      });
    }

    return false;
  }

  void _showPermissionDeniedDialog() {
    showDialog(
      context: context,
      builder: (context) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: const Text(
          'Bluetooth Permission',
          style: TextStyle(color: CelestialColors.textPrimary),
        ),
        content: const Text(
          'Bluetooth permission was denied. Please enable it in Settings to set up your RhythmBox.',
          style: TextStyle(color: CelestialColors.textSecondary),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(),
            child: Text(
              'Cancel',
              style: TextStyle(color: CelestialColors.textSecondary),
            ),
          ),
          TextButton(
            onPressed: () {
              Navigator.of(context).pop();
              openAppSettings();
            },
            child: const Text(
              'Open Settings',
              style: TextStyle(color: Color(0xFF00BCD4)),
            ),
          ),
        ],
      ),
    );
  }

  // ─── Scan & Connect Logic ─────────────────────────────────

  Future<void> _startScan() async {
    final granted = await _ensureBluetoothPermission();
    if (!granted || !mounted) return;

    final bluetoothOn = await _bleService.isBluetoothOn();
    if (!bluetoothOn) {
      if (!mounted) return;
      setState(() {
        _status = Esp32ProvisioningStatus.error;
        _errorMessage = 'Bluetooth is turned off. Please enable Bluetooth in Settings.';
      });
      return;
    }

    setState(() {
      _status = Esp32ProvisioningStatus.scanning;
      _errorMessage = null;
      _discoveredDevices = [];
      _selectedDevice = null;
    });

    _scanSubscription?.cancel();
    _scanSubscription = _bleService.scanForDevices().listen(
      (device) {
        if (!mounted) return;
        setState(() {
          _discoveredDevices.add(device);
          _status = Esp32ProvisioningStatus.deviceFound;
        });
      },
      onError: (e) {
        if (!mounted) return;
        setState(() {
          _status = Esp32ProvisioningStatus.error;
          _errorMessage = 'Scan failed: $e';
        });
      },
    );
  }

  /// iOS path: show the AccessorySetupKit picker, then connect by UUID.
  Future<void> _startSetupWithASK() async {
    setState(() {
      _status = Esp32ProvisioningStatus.connecting;
      _errorMessage = null;
    });

    try {
      final bluetoothId = await _accessoryService.showPicker();
      debugPrint('[ASK] showPicker returned: $bluetoothId');
      if (!mounted) return;

      if (bluetoothId == null) {
        // User cancelled the picker
        setState(() {
          _status = Esp32ProvisioningStatus.ready;
        });
        return;
      }

      // Brief delay — ASK disconnects after pairing, give ESP32 time to
      // restart advertising before flutter_blue_plus reconnects.
      await Future<void>.delayed(const Duration(milliseconds: 500));

      // Wait for CoreBluetooth to be ready — CBManagerState may still be
      // "unknown" right after the ASK picker dismisses.
      final bluetoothOn = await _bleService.isBluetoothOn();
      if (!bluetoothOn) {
        if (!mounted) return;
        setState(() {
          _status = Esp32ProvisioningStatus.error;
          _errorMessage = 'Bluetooth is not ready. Please ensure Bluetooth is enabled and try again.';
        });
        return;
      }

      debugPrint('[ASK] Connecting via flutter_blue_plus to $bluetoothId');
      _deviceInfo = await _bleService.connectById(bluetoothId);
      if (!mounted) return;

      if (!mounted) return;
      setState(() {
        _status = Esp32ProvisioningStatus.wifiSetup;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _status = Esp32ProvisioningStatus.error;
        _errorMessage = 'Failed to connect: $e';
      });
    }
  }

  Future<void> _connectToDevice(BleDevice device) async {
    setState(() {
      _selectedDevice = device;
      _status = Esp32ProvisioningStatus.connecting;
      _errorMessage = null;
    });

    try {
      _deviceInfo = await _bleService.connect(device);
      if (!mounted) return;

      if (!mounted) return;
      setState(() {
        _status = Esp32ProvisioningStatus.wifiSetup;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _status = Esp32ProvisioningStatus.error;
        _errorMessage = 'Failed to connect: $e';
      });
    }
  }

  Future<void> _sendWifiCredentials() async {
    final ssid = _ssidController.text.trim();
    final password = _passwordController.text;

    if (ssid.isEmpty) return;

    AnalyticsService().logEsp32ProvisioningStarted();

    setState(() {
      _status = Esp32ProvisioningStatus.provisioning;
      _errorMessage = null;
      _wifiErrorMessage = null;
    });

    try {
      final ip = await _bleService.sendWifiCredentials(ssid, password);
      if (!mounted) return;

      setState(() {
        _provisionedIp = ip;
        // Stay in provisioning — wait for WS connection
      });

      HapticFeedback.heavyImpact();

      // Persist the hub — triggers WS auto-connect via ProxyProvider
      if (mounted) {
        await context.read<HomeProvider>().addServerHub(
          name: _deviceInfo?.name ?? 'RhythmBox',
          host: ip,
        );
      }

      // Wait for HTTP connection (ESP32 is still booting)
      if (mounted) {
        final conn = context.read<RhythmConnection>();
        final connected = await _waitForConnection(conn,
            timeout: const Duration(seconds: 30));

        if (!mounted) return;

        if (connected) {
          AnalyticsService().logEsp32ProvisioningCompleted();
          setState(() {
            _status = Esp32ProvisioningStatus.success;
          });
        } else {
          AnalyticsService().logEsp32ProvisioningFailed(
            'HTTP connection failed after WiFi provisioning',
          );
          setState(() {
            _status = Esp32ProvisioningStatus.error;
            _errorMessage =
                'RhythmBox joined WiFi but could not establish communication. '
                'Make sure your phone is on the same network.';
          });
        }
      }
    } on WifiFailedException catch (e) {
      // Wrong password — BLE is still alive, go back to WiFi form
      if (!mounted) return;
      _passwordController.clear();
      setState(() {
        _status = Esp32ProvisioningStatus.wifiSetup;
        _wifiErrorMessage = 'WiFi connection failed: ${e.message}. Check your password and try again.';
      });
    } catch (e) {
      if (!mounted) return;
      AnalyticsService().logEsp32ProvisioningFailed(e.toString());
      // Timeout or other error — check if BLE is still alive
      if (_bleService.isConnected) {
        // BLE alive: let the user retry from the WiFi form
        _passwordController.clear();
        setState(() {
          _status = Esp32ProvisioningStatus.wifiSetup;
          _wifiErrorMessage = 'Connection timed out. Check your password and try again.';
        });
      } else {
        // BLE dead: need full restart
        setState(() {
          _status = Esp32ProvisioningStatus.error;
          _errorMessage = 'Provisioning failed: $e';
        });
      }
    }
  }

  /// Wait for the connection to reach connected state.
  Future<bool> _waitForConnection(
    RhythmConnection conn, {
    required Duration timeout,
  }) async {
    if (conn.connected) return true;

    final completer = Completer<bool>();

    final sub = conn.connectionStateStream.listen((state) {
      if (state == RhythmConnectionState.connected && !completer.isCompleted) {
        completer.complete(true);
      }
    });
    final timer = Timer(timeout, () {
      if (!completer.isCompleted) {
        completer.complete(false);
      }
    });

    try {
      return await completer.future;
    } finally {
      sub.cancel();
      timer.cancel();
    }
  }

  Future<void> _onComplete() async {
    await SuccessModal.show(
      context,
      roomCount: 0,
      hubType: 'RhythmBox',
    );

    if (mounted) {
      Navigator.of(context).popUntil((route) => route.isFirst);
    }
  }

  Future<void> _startOver() async {
    await _bleService.disconnect();
    if (_useASK) {
      await _accessoryService.removeAccessory();
    }
    if (!mounted) return;
    setState(() {
      _status = Esp32ProvisioningStatus.ready;
      _errorMessage = null;
      _wifiErrorMessage = null;
      _discoveredDevices = [];
      _selectedDevice = null;
      _deviceInfo = null;
      _provisionedIp = null;
    });
  }

  // ─── Build ────────────────────────────────────────────────

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
                    _buildHeroSection(),
                    const SizedBox(height: 32),
                    _buildContent(),
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
          const Expanded(
            child: Text(
              'RhythmBox',
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

  // ─── Hero Section ─────────────────────────────────────────

  Widget _buildHeroSection() {
    return AnimatedBuilder(
      animation: _glowAnimation,
      builder: (context, child) {
        final isActive = _status == Esp32ProvisioningStatus.scanning ||
            _status == Esp32ProvisioningStatus.connecting ||
            _status == Esp32ProvisioningStatus.provisioning;
        final glowIntensity = isActive ? 0.8 : _glowAnimation.value;

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
              color: _teal.withValues(alpha: 0.2),
              width: 1,
            ),
          ),
          child: Column(
            children: [
              Stack(
                alignment: Alignment.center,
                children: [
                  if (isActive)
                    SizedBox(
                      width: 88,
                      height: 88,
                      child: CircularProgressIndicator(
                        strokeWidth: 4,
                        backgroundColor:
                            CelestialColors.orbitRing.withValues(alpha: 0.3),
                        valueColor: const AlwaysStoppedAnimation(_teal),
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
                        colors: [_teal, _tealDeep],
                      ),
                      boxShadow: [
                        BoxShadow(
                          color: _teal.withValues(alpha: glowIntensity),
                          blurRadius: isActive ? 32 : 24,
                          spreadRadius: isActive ? 4 : 2,
                        ),
                      ],
                    ),
                    child: Icon(
                      _status == Esp32ProvisioningStatus.success
                          ? Icons.check_rounded
                          : Icons.developer_board,
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
            ],
          ),
        );
      },
    );
  }

  String _getHeroTitle() {
    switch (_status) {
      case Esp32ProvisioningStatus.ready:
        return 'Set up your RhythmBox';
      case Esp32ProvisioningStatus.scanning:
        return 'Scanning for devices...';
      case Esp32ProvisioningStatus.deviceFound:
        return 'Device found';
      case Esp32ProvisioningStatus.connecting:
        return 'Connecting...';
      case Esp32ProvisioningStatus.wifiSetup:
        return _wifiErrorMessage != null ? 'WiFi Failed' : 'Configure WiFi';
      case Esp32ProvisioningStatus.provisioning:
        return _provisionedIp != null ? 'Connecting to RhythmBox...' : 'Configuring WiFi...';
      case Esp32ProvisioningStatus.success:
        return 'RhythmBox ready';
      case Esp32ProvisioningStatus.error:
        return 'Setup failed';
    }
  }

  String _getHeroSubtitle() {
    switch (_status) {
      case Esp32ProvisioningStatus.ready:
        return 'Connect your RhythmBox to your WiFi network via Bluetooth';
      case Esp32ProvisioningStatus.scanning:
        return 'Looking for nearby RhythmBox devices';
      case Esp32ProvisioningStatus.deviceFound:
        return 'Found ${_discoveredDevices.length} device${_discoveredDevices.length == 1 ? '' : 's'} nearby';
      case Esp32ProvisioningStatus.connecting:
        return 'Connecting to ${_selectedDevice?.name ?? 'device'}';
      case Esp32ProvisioningStatus.wifiSetup:
        if (_wifiErrorMessage != null) {
          return 'Re-enter your WiFi password and try again';
        }
        final version = _deviceInfo != null ? ' (v${_deviceInfo!.version})' : '';
        return 'Enter your WiFi credentials to connect ${_selectedDevice?.name ?? 'your RhythmBox'}$version';
      case Esp32ProvisioningStatus.provisioning:
        return _provisionedIp != null
            ? 'Waiting for RhythmBox to come online at $_provisionedIp'
            : 'Sending WiFi credentials to ${_selectedDevice?.name ?? 'your RhythmBox'}';
      case Esp32ProvisioningStatus.success:
        if (_provisionedIp != null) {
          return 'Your controller is online at $_provisionedIp';
        }
        return 'Your controller is connecting to WiFi';
      case Esp32ProvisioningStatus.error:
        return _errorMessage ?? 'An error occurred';
    }
  }

  // ─── Content by State ─────────────────────────────────────

  Widget _buildContent() {
    switch (_status) {
      case Esp32ProvisioningStatus.ready:
        return _buildReadyContent();
      case Esp32ProvisioningStatus.scanning:
        return _buildScanningContent();
      case Esp32ProvisioningStatus.deviceFound:
        return _buildDeviceFoundContent();
      case Esp32ProvisioningStatus.connecting:
        return _buildProgressContent('Establishing Bluetooth connection...');
      case Esp32ProvisioningStatus.wifiSetup:
        return _buildWifiSetupContent();
      case Esp32ProvisioningStatus.provisioning:
        return _buildProgressContent('Sending WiFi credentials...');
      case Esp32ProvisioningStatus.success:
        return _buildSuccessContent();
      case Esp32ProvisioningStatus.error:
        return _buildErrorContent();
    }
  }

  Widget _buildReadyContent() {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _buildActionButton(
          onTap: _useASK ? _startSetupWithASK : _startScan,
          icon: _useASK
              ? Icons.devices_rounded
              : Icons.bluetooth_searching_rounded,
          label: _useASK ? 'Set up RhythmBox' : 'Scan for RhythmBox',
          isPrimary: true,
        ),
        const SizedBox(height: 24),
        _buildInfoCard(
          icon: Icons.info_outline_rounded,
          title: 'Before you begin',
          description:
              'Make sure your RhythmBox is powered on and in pairing mode. The LED should be blinking blue.',
        ),
      ],
    );
  }

  Widget _buildScanningContent() {
    return Column(
      children: [
        const SizedBox(height: 24),
        const SizedBox(
          width: 48,
          height: 48,
          child: CircularProgressIndicator(
            strokeWidth: 3,
            valueColor: AlwaysStoppedAnimation(_teal),
          ),
        ),
        const SizedBox(height: 16),
        Text(
          'Scanning for devices...',
          style: TextStyle(
            color: CelestialColors.textSecondary,
            fontSize: 14,
          ),
        ),
      ],
    );
  }

  Widget _buildDeviceFoundContent() {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        ..._discoveredDevices.map((device) => _buildDeviceCard(device)),
        const SizedBox(height: 24),
        _buildActionButton(
          onTap: _startScan,
          icon: Icons.refresh_rounded,
          label: 'Scan Again',
          isPrimary: false,
        ),
      ],
    );
  }

  Widget _buildDeviceCard(BleDevice device) {
    // Signal strength indicator
    final signalBars = _rssiToSignalBars(device.rssi);

    return GestureDetector(
      onTap: () {
        HapticFeedback.selectionClick();
        _connectToDevice(device);
      },
      child: Container(
        margin: const EdgeInsets.only(bottom: 12),
        padding: const EdgeInsets.all(16),
        decoration: BoxDecoration(
          color: CelestialColors.backgroundCard,
          borderRadius: BorderRadius.circular(14),
          border: Border.all(
            color: CelestialColors.orbitRing.withValues(alpha: 0.5),
          ),
        ),
        child: Row(
          children: [
            Container(
              width: 44,
              height: 44,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: _teal.withValues(alpha: 0.15),
              ),
              child: const Icon(
                Icons.developer_board,
                color: _teal,
                size: 22,
              ),
            ),
            const SizedBox(width: 14),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    device.name,
                    style: const TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 15,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    '${device.rssi} dBm',
                    style: TextStyle(
                      color: CelestialColors.textSecondary
                          .withValues(alpha: 0.8),
                      fontSize: 13,
                      fontFamily: 'monospace',
                    ),
                  ),
                ],
              ),
            ),
            // Signal strength bars
            Row(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.end,
              children: List.generate(4, (i) {
                final isActive = i < signalBars;
                return Container(
                  width: 4,
                  height: 6.0 + i * 4,
                  margin: const EdgeInsets.only(left: 2),
                  decoration: BoxDecoration(
                    borderRadius: BorderRadius.circular(1),
                    color: isActive
                        ? _teal
                        : CelestialColors.textSecondary
                            .withValues(alpha: 0.2),
                  ),
                );
              }),
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
    );
  }

  int _rssiToSignalBars(int rssi) {
    if (rssi >= -50) return 4;
    if (rssi >= -65) return 3;
    if (rssi >= -80) return 2;
    return 1;
  }

  Widget _buildWifiSetupContent() {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // Warning banner for WiFi retry
        if (_wifiErrorMessage != null) ...[
          Container(
            padding: const EdgeInsets.all(14),
            decoration: BoxDecoration(
              color: Colors.orange.shade700.withValues(alpha: 0.12),
              borderRadius: BorderRadius.circular(14),
              border: Border.all(
                color: Colors.orange.shade700.withValues(alpha: 0.3),
              ),
            ),
            child: Row(
              children: [
                Icon(Icons.warning_amber_rounded,
                    color: Colors.orange.shade400, size: 20),
                const SizedBox(width: 12),
                Expanded(
                  child: Text(
                    _wifiErrorMessage!,
                    style: TextStyle(
                      color: Colors.orange.shade400,
                      fontSize: 13,
                      fontWeight: FontWeight.w500,
                      height: 1.4,
                    ),
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(height: 16),
        ],
        // SSID field
        Container(
          decoration: BoxDecoration(
            color: CelestialColors.backgroundCard,
            borderRadius: BorderRadius.circular(14),
            border: Border.all(
              color: CelestialColors.orbitRing.withValues(alpha: 0.5),
            ),
          ),
          child: TextField(
            controller: _ssidController,
            style: const TextStyle(
              color: CelestialColors.textPrimary,
              fontSize: 15,
            ),
            decoration: InputDecoration(
              labelText: 'WiFi Network',
              labelStyle: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.7),
              ),
              prefixIcon: Icon(
                Icons.wifi_rounded,
                color: _teal.withValues(alpha: 0.7),
                size: 20,
              ),
              border: InputBorder.none,
              contentPadding: const EdgeInsets.symmetric(
                horizontal: 16,
                vertical: 16,
              ),
            ),
          ),
        ),
        const SizedBox(height: 12),
        // Password field
        Container(
          decoration: BoxDecoration(
            color: CelestialColors.backgroundCard,
            borderRadius: BorderRadius.circular(14),
            border: Border.all(
              color: CelestialColors.orbitRing.withValues(alpha: 0.5),
            ),
          ),
          child: TextField(
            controller: _passwordController,
            obscureText: _obscurePassword,
            style: const TextStyle(
              color: CelestialColors.textPrimary,
              fontSize: 15,
            ),
            decoration: InputDecoration(
              labelText: 'Password',
              labelStyle: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.7),
              ),
              prefixIcon: Icon(
                Icons.lock_outline_rounded,
                color: _teal.withValues(alpha: 0.7),
                size: 20,
              ),
              suffixIcon: GestureDetector(
                onTap: () {
                  setState(() {
                    _obscurePassword = !_obscurePassword;
                  });
                },
                child: Icon(
                  _obscurePassword
                      ? Icons.visibility_off_rounded
                      : Icons.visibility_rounded,
                  color: CelestialColors.textSecondary.withValues(alpha: 0.5),
                  size: 20,
                ),
              ),
              border: InputBorder.none,
              contentPadding: const EdgeInsets.symmetric(
                horizontal: 16,
                vertical: 16,
              ),
            ),
          ),
        ),
        const SizedBox(height: 24),
        _buildActionButton(
          onTap: _ssidController.text.trim().isNotEmpty
              ? _sendWifiCredentials
              : null,
          icon: Icons.wifi_protected_setup_rounded,
          label: 'Connect to WiFi',
          isPrimary: true,
        ),
        const SizedBox(height: 12),
        _buildActionButton(
          onTap: _startOver,
          icon: Icons.refresh_rounded,
          label: 'Start Over',
          isPrimary: false,
        ),
      ],
    );
  }

  Widget _buildProgressContent(String label) {
    return Column(
      children: [
        const SizedBox(height: 24),
        const SizedBox(
          width: 48,
          height: 48,
          child: CircularProgressIndicator(
            strokeWidth: 3,
            valueColor: AlwaysStoppedAnimation(_teal),
          ),
        ),
        const SizedBox(height: 16),
        Text(
          label,
          style: TextStyle(
            color: CelestialColors.textSecondary,
            fontSize: 14,
          ),
        ),
      ],
    );
  }

  Widget _buildSuccessContent() {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // Credentials sent card
        Container(
          padding: const EdgeInsets.all(16),
          decoration: BoxDecoration(
            color: const Color(0xFF22C55E).withValues(alpha: 0.1),
            borderRadius: BorderRadius.circular(14),
            border: Border.all(
              color: const Color(0xFF22C55E).withValues(alpha: 0.3),
            ),
          ),
          child: Row(
            children: [
              Container(
                width: 36,
                height: 36,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: const Color(0xFF22C55E).withValues(alpha: 0.2),
                ),
                child: const Icon(
                  Icons.check_circle_rounded,
                  color: Color(0xFF22C55E),
                  size: 20,
                ),
              ),
              const SizedBox(width: 14),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    const Text(
                      'Credentials sent',
                      style: TextStyle(
                        color: CelestialColors.textPrimary,
                        fontSize: 14,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      _provisionedIp != null
                          ? 'Your controller is online at $_provisionedIp.'
                          : 'Your controller is connecting to WiFi. Add it in Settings once it\'s online.',
                      style: TextStyle(
                        color: CelestialColors.textSecondary
                            .withValues(alpha: 0.8),
                        fontSize: 13,
                        height: 1.4,
                      ),
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
        const SizedBox(height: 24),
        _buildActionButton(
          onTap: _onComplete,
          icon: Icons.check_rounded,
          label: 'Done',
          isPrimary: true,
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
                  _errorMessage ?? 'An error occurred',
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
          onTap: _startOver,
          icon: Icons.refresh_rounded,
          label: 'Try Again',
          isPrimary: true,
        ),
      ],
    );
  }

  // ─── Shared UI Components ─────────────────────────────────

  Widget _buildActionButton({
    required VoidCallback? onTap,
    required IconData icon,
    required String label,
    required bool isPrimary,
  }) {
    final isEnabled = onTap != null;

    return GestureDetector(
      onTap: onTap,
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 200),
        padding: const EdgeInsets.symmetric(vertical: 16),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(14),
          gradient: isPrimary && isEnabled
              ? const LinearGradient(
                  begin: Alignment.topLeft,
                  end: Alignment.bottomRight,
                  colors: [_teal, _tealDeep],
                )
              : null,
          color: isPrimary
              ? (isEnabled
                  ? null
                  : CelestialColors.orbitRing.withValues(alpha: 0.3))
              : CelestialColors.backgroundCard,
          border: isPrimary
              ? null
              : Border.all(
                  color: CelestialColors.orbitRing.withValues(alpha: 0.5),
                ),
          boxShadow: isPrimary && isEnabled
              ? [
                  BoxShadow(
                    color: _teal.withValues(alpha: 0.3),
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
                  : CelestialColors.textSecondary,
              size: 20,
            ),
            const SizedBox(width: 10),
            Text(
              label,
              style: TextStyle(
                color: isPrimary
                    ? (isEnabled
                        ? Colors.white
                        : CelestialColors.textSecondary
                            .withValues(alpha: 0.5))
                    : CelestialColors.textPrimary,
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
              color: _teal.withValues(alpha: 0.15),
            ),
            child: Icon(
              icon,
              color: _teal,
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
                    color: CelestialColors.textSecondary
                        .withValues(alpha: 0.8),
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
