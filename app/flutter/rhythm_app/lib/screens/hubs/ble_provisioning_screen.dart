import 'dart:async';
import 'dart:io' show Platform;

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:permission_handler/permission_handler.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show RhythmAuthApi, RhythmDiagnosticsApi;

import '../../providers/home_provider.dart';
import '../../services/analytics_service.dart';
import '../../services/ble_provisioning_service.dart';
import '../../services/recent_servers_service.dart';
import '../../widgets/solar_orbit.dart';

enum _ProvisioningPhase {
  scanning,
  devices,
  connecting,
  credentials,
  provisioning,
  success,
  error,
}

class BleProvisioningScreen extends StatefulWidget {
  const BleProvisioningScreen({super.key, this.initialDevice});

  final BleDevice? initialDevice;

  static Future<void> show(BuildContext context, {BleDevice? initialDevice}) {
    return Navigator.of(context).push(
      PageRouteBuilder<void>(
        opaque: false,
        barrierColor: Colors.black54,
        pageBuilder: (context, animation, secondaryAnimation) {
          return BleProvisioningScreen(initialDevice: initialDevice);
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
  State<BleProvisioningScreen> createState() => _BleProvisioningScreenState();
}

class _BleProvisioningScreenState extends State<BleProvisioningScreen>
    with SingleTickerProviderStateMixin {
  static const _teal = Color(0xFF00BCD4);
  static const _tealDeep = Color(0xFF0097A7);

  final _bleService = BleProvisioningService();
  final _ssidController = TextEditingController();
  final _passwordController = TextEditingController();

  StreamSubscription<BleDevice>? _scanSubscription;
  late final AnimationController _glowController;
  late final Animation<double> _glowAnimation;

  _ProvisioningPhase _phase = _ProvisioningPhase.devices;
  final List<BleDevice> _devices = <BleDevice>[];
  BleDevice? _selectedDevice;
  BleDeviceInfo? _deviceInfo;
  String? _provisionedIp;
  String? _errorMessage;
  String? _wifiErrorMessage;
  bool _obscurePassword = true;
  bool _hasAttemptedScan = false;

  bool get _supportsBleProvisioning {
    if (kIsWeb) return false;
    return defaultTargetPlatform == TargetPlatform.iOS ||
        defaultTargetPlatform == TargetPlatform.macOS;
  }

  String get _selectedDeviceName =>
      _selectedDevice?.name ?? _deviceInfo?.name ?? 'Rhythm Box';

  @override
  void initState() {
    super.initState();
    _glowController = AnimationController(
      duration: const Duration(milliseconds: 2200),
      vsync: this,
    )..repeat(reverse: true);
    _glowAnimation = CurvedAnimation(
      parent: _glowController,
      curve: Curves.easeInOut,
    );

    _ssidController.addListener(_onWifiFieldChanged);
    _passwordController.addListener(_onWifiFieldChanged);
    AnalyticsService().logScreenView('ble_provisioning');

    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!_supportsBleProvisioning) {
        setState(() {
          _phase = _ProvisioningPhase.error;
          _errorMessage =
              'Device setup is only available on the native mobile and desktop builds.';
        });
        return;
      }
      setState(() {
        _phase = _ProvisioningPhase.devices;
        _errorMessage = null;
        _wifiErrorMessage = null;
        _devices.clear();
        _selectedDevice = null;
        _deviceInfo = null;
        _provisionedIp = null;
        _hasAttemptedScan = false;
      });
      final initialDevice = widget.initialDevice;
      if (initialDevice != null) {
        unawaited(_connectToDevice(initialDevice));
      }
    });
  }

  @override
  void dispose() {
    _ssidController.removeListener(_onWifiFieldChanged);
    _passwordController.removeListener(_onWifiFieldChanged);
    _scanSubscription?.cancel();
    _bleService.dispose();
    _ssidController.dispose();
    _passwordController.dispose();
    _glowController.dispose();
    super.dispose();
  }

  void _onWifiFieldChanged() {
    if (mounted && _phase == _ProvisioningPhase.credentials) {
      setState(() {});
    }
  }

  Future<bool> _ensureBluetoothPermission() async {
    if (kIsWeb || Platform.isMacOS) return true;

    if (Platform.isIOS) {
      final permission = Permission.bluetooth;
      final status = await permission.status;
      debugPrint('[BLE Permission] before scan: $permission => $status');

      if (status.isGranted) return true;

      if (!mounted) return false;

      if (status.isPermanentlyDenied) {
        _showPermissionDeniedDialog();
        return false;
      }

      if (status.isRestricted) {
        setState(() {
          _phase = _ProvisioningPhase.error;
          _errorMessage = 'Bluetooth access is restricted on this device.';
        });
        return false;
      }

      // On iOS, let CoreBluetooth resolve the undecided state during the
      // first real BLE operation instead of blocking on a preflight check.
      return true;
    }

    final permissions = <Permission>[];
    if (Platform.isAndroid) {
      permissions.addAll([
        Permission.bluetoothScan,
        Permission.bluetoothConnect,
        Permission.locationWhenInUse,
      ]);
    }

    if (permissions.isEmpty) return true;

    for (final permission in permissions) {
      final status = await permission.status;
      debugPrint('[BLE Permission] before request: $permission => $status');
    }

    var statuses = await permissions.request();
    statuses.forEach((permission, status) {
      debugPrint('[BLE Permission] request result: $permission => $status');
    });
    var denied =
        statuses.entries.where((entry) => !entry.value.isGranted).toList();

    if (denied.isEmpty) return true;

    if (!mounted) return false;

    if (denied.any((entry) => entry.value.isPermanentlyDenied)) {
      _showPermissionDeniedDialog();
    } else {
      setState(() {
        _phase = _ProvisioningPhase.error;
        _errorMessage =
            'Bluetooth permission is required to find nearby Rhythm Boxes.';
      });
    }
    return false;
  }

  void _showPermissionDeniedDialog() {
    showDialog<void>(
      context: context,
      builder: (context) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: const Text(
          'Bluetooth Permission',
          style: TextStyle(color: CelestialColors.textPrimary),
        ),
        content: const Text(
          'Bluetooth permission was denied. Enable it in Settings to set up your Rhythm Box.',
          style: TextStyle(color: CelestialColors.textSecondary),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(),
            child: Text(
              'Cancel',
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.82),
              ),
            ),
          ),
          TextButton(
            onPressed: () {
              Navigator.of(context).pop();
              openAppSettings();
            },
            child: const Text(
              'Open Settings',
              style: TextStyle(color: _teal),
            ),
          ),
        ],
      ),
    );
  }

  Future<void> _startScan() async {
    await _scanSubscription?.cancel();
    _bleService.stopScan();

    final granted = await _ensureBluetoothPermission();
    if (!granted || !mounted) return;

    if (!Platform.isIOS) {
      final bluetoothOn = await _bleService.isBluetoothOn();
      if (!mounted) return;
      if (!bluetoothOn) {
        setState(() {
          _phase = _ProvisioningPhase.error;
          _errorMessage =
              'Bluetooth is turned off. Please enable Bluetooth in Settings.';
        });
        return;
      }
    }

    setState(() {
      _phase = _ProvisioningPhase.scanning;
      _errorMessage = null;
      _wifiErrorMessage = null;
      _devices.clear();
      _selectedDevice = null;
      _deviceInfo = null;
      _provisionedIp = null;
      _hasAttemptedScan = true;
    });

    _scanSubscription = _bleService.scanForDevices().listen(
      (device) {
        if (!mounted) return;
        setState(() {
          _devices.add(device);
          _phase = _ProvisioningPhase.devices;
        });
      },
      onError: (error) {
        unawaited(_handleScanError(error));
      },
    );

    unawaited(_awaitScanCompletion());
  }

  Future<void> _handleScanError(Object error) async {
    if (!mounted) return;

    if (Platform.isIOS) {
      final permissionStatus = await Permission.bluetooth.status;
      debugPrint(
        '[BLE Permission] scan error status: Permission.bluetooth => $permissionStatus',
      );
      if (!mounted) return;

      if (permissionStatus.isPermanentlyDenied) {
        _showPermissionDeniedDialog();
        return;
      }

      final message = '$error';
      if (message.contains('CBManagerStatePoweredOff') ||
          message.contains('BluetoothAdapterState.off')) {
        setState(() {
          _phase = _ProvisioningPhase.error;
          _errorMessage =
              'Bluetooth is turned off. Please enable Bluetooth in Settings.';
        });
        return;
      }

      if (message.contains('CBManagerStateUnauthorized') ||
          message.contains('BluetoothAdapterState.unauthorized')) {
        setState(() {
          _phase = _ProvisioningPhase.error;
          _errorMessage =
              'Bluetooth permission was denied. Enable it in Settings to continue.';
        });
        return;
      }

      if (message.contains('BluetoothAdapterState.unavailable')) {
        setState(() {
          _phase = _ProvisioningPhase.error;
          _errorMessage =
              'This device does not support Bluetooth LE provisioning.';
        });
        return;
      }

      if (message.contains('BluetoothAdapterState.unknown') ||
          error is TimeoutException) {
        setState(() {
          _phase = _ProvisioningPhase.error;
          _errorMessage =
              'Bluetooth is still initializing. Wait a moment and try scanning again.';
        });
        return;
      }

      setState(() {
        _phase = _ProvisioningPhase.error;
        _errorMessage = permissionStatus.isDenied
            ? 'Bluetooth permission did not complete. If no prompt appeared, try deleting and reinstalling the app or resetting Location & Privacy.'
            : 'Device search failed: $error';
      });
      return;
    }

    setState(() {
      _phase = _ProvisioningPhase.error;
      _errorMessage = 'Device search failed: $error';
    });
  }

  Future<void> _awaitScanCompletion() async {
    try {
      await _bleService.waitForScanToFinish();
      if (!mounted) return;
      if (_phase == _ProvisioningPhase.scanning) {
        setState(() {
          _phase = _ProvisioningPhase.devices;
        });
      }
    } catch (_) {
      // Ignore scan lifecycle errors. The main subscription handles surfaced failures.
    }
  }

  Future<void> _connectToDevice(BleDevice device) async {
    HapticFeedback.mediumImpact();
    _bleService.stopScan();

    setState(() {
      _selectedDevice = device;
      _errorMessage = null;
      _wifiErrorMessage = null;
      _phase = _ProvisioningPhase.connecting;
    });

    try {
      final info = await _bleService.connect(device);
      if (!mounted) return;

      setState(() {
        _deviceInfo = info;
        _phase = _ProvisioningPhase.credentials;
      });
    } catch (error) {
      if (!mounted) return;
      setState(() {
        _phase = _ProvisioningPhase.error;
        _errorMessage = 'Couldn\'t connect to ${device.name}. $error';
      });
    }
  }

  Future<void> _submitCredentials() async {
    final ssid = _ssidController.text.trim();
    final password = _passwordController.text;
    if (ssid.isEmpty) return;

    AnalyticsService().logEvent('ble_provisioning_started');
    HapticFeedback.mediumImpact();

    setState(() {
      _phase = _ProvisioningPhase.provisioning;
      _errorMessage = null;
      _wifiErrorMessage = null;
      _provisionedIp = null;
    });

    try {
      final result = await _bleService.sendWifiCredentials(ssid, password);
      final ip = result.ip;
      if (!mounted) return;

      setState(() {
        _provisionedIp = ip;
      });

      final online = await _waitForServerHealth(ip);
      if (!mounted) return;
      if (!online) {
        AnalyticsService().logEvent('ble_provisioning_failed', {
          'stage': 'health_timeout',
        });
        setState(() {
          _phase = _ProvisioningPhase.error;
          _errorMessage =
              'Your Rhythm Box joined Wi-Fi at $ip but didn\'t come online in time. Make sure you\'re on the same network and try again.';
        });
        return;
      }

      final ownerToken = await _resolveOwnerTokenAfterProvisioning(
        ip,
        result.ownerToken,
      );
      await _persistServerHub(ip, ownerToken);
      if (!mounted) return;

      AnalyticsService().logEvent('ble_provisioning_completed');
      HapticFeedback.heavyImpact();
      setState(() {
        _phase = _ProvisioningPhase.success;
      });
    } on WifiFailedException catch (error) {
      if (!mounted) return;
      _passwordController.clear();
      setState(() {
        _phase = _ProvisioningPhase.credentials;
        _wifiErrorMessage = error.message;
      });
    } catch (error) {
      if (!mounted) return;
      AnalyticsService().logEvent('ble_provisioning_failed', {
        'stage': 'runtime',
      });
      if (_bleService.isConnected) {
        _passwordController.clear();
        setState(() {
          _phase = _ProvisioningPhase.credentials;
          _wifiErrorMessage = '$error';
        });
      } else {
        setState(() {
          _phase = _ProvisioningPhase.error;
          _errorMessage = 'Setup failed: $error';
        });
      }
    }
  }

  Future<bool> _waitForServerHealth(String ip) async {
    final client = RhythmDiagnosticsApi(host: ip, port: 54448);
    final deadline = DateTime.now().add(const Duration(seconds: 30));

    while (DateTime.now().isBefore(deadline)) {
      if (await client.healthCheck()) {
        return true;
      }
      await Future<void>.delayed(const Duration(seconds: 1));
    }
    return false;
  }

  Future<String?> _resolveOwnerTokenAfterProvisioning(
    String ip,
    String? provisionedOwnerToken,
  ) async {
    final provisioned = provisionedOwnerToken?.trim();
    if (provisioned != null && provisioned.isNotEmpty) return provisioned;

    try {
      final status = await RhythmAuthApi(
        baseUrl: 'http://$ip:54448',
      ).getStatus();
      if (!status.requiresAuth) {
        return null;
      }
    } catch (error) {
      debugPrint('[BLE] auth status after provisioning failed: $error');
    }

    if (!_bleService.isConnected) {
      throw StateError('Server requires API auth, but Bluetooth disconnected.');
    }

    return _bleService.requestOwnerToken(label: 'Rhythm app');
  }

  Future<void> _persistServerHub(String ip, String? ownerToken) async {
    final homeProvider = context.read<HomeProvider>();
    final displayName = _deviceInfo?.name ?? 'RhythmServer';
    for (final hub in homeProvider.currentHomeHubs) {
      final matches = hub.type == HubType.server &&
          hub.endpoint.host == ip &&
          hub.endpoint.port == 54448;
      if (!matches) continue;

      final token = ownerToken?.trim();
      final hasSavedToken = hub.token?.trim().isNotEmpty == true;
      var selectedHub = hub;
      if (token != null && token.isNotEmpty && !hasSavedToken) {
        selectedHub = hub.copyWith(token: token);
      }
      await homeProvider.activateServerHub(selectedHub);
      await RecentServersService.instance.record(
        name: hub.name,
        host: ip,
        port: 54448,
        token: token != null && token.isNotEmpty ? token : hub.token,
      );
      return;
    }

    final hub = await homeProvider.addServerHub(
      name: displayName,
      host: ip,
      port: 54448,
      token: ownerToken,
    );
    if (hub != null) {
      await homeProvider.activateServerHub(hub);
    }
    await RecentServersService.instance.record(
      name: displayName,
      host: ip,
      port: 54448,
      token: ownerToken,
    );
  }

  Future<void> _startOver() async {
    _ssidController.clear();
    _passwordController.clear();
    await _bleService.disconnect();
    if (!mounted) return;
    setState(() {
      _phase = _ProvisioningPhase.devices;
      _errorMessage = null;
      _wifiErrorMessage = null;
      _devices.clear();
      _selectedDevice = null;
      _deviceInfo = null;
      _provisionedIp = null;
      _hasAttemptedScan = false;
    });
  }

  void _close() {
    Navigator.of(context).pop();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      appBar: AppBar(
        backgroundColor: Colors.transparent,
        foregroundColor: CelestialColors.textPrimary,
        elevation: 0,
        title: const Text('Set Up New Box'),
      ),
      body: SafeArea(
        top: false,
        child: AnimatedBuilder(
          animation: _glowAnimation,
          builder: (context, _) {
            return SingleChildScrollView(
              padding: const EdgeInsets.fromLTRB(24, 12, 24, 24),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  _buildHero(),
                  const SizedBox(height: 24),
                  _buildHeader(),
                  const SizedBox(height: 24),
                  _buildBody(),
                ],
              ),
            );
          },
        ),
      ),
    );
  }

  Widget _buildHero() {
    final glow = _glowAnimation.value;
    return Center(
      child: Container(
        width: 108,
        height: 108,
        decoration: BoxDecoration(
          shape: BoxShape.circle,
          gradient: LinearGradient(
            begin: Alignment.topLeft,
            end: Alignment.bottomRight,
            colors: [
              _teal.withValues(alpha: 0.18 + glow * 0.06),
              _tealDeep.withValues(alpha: 0.12 + glow * 0.08),
            ],
          ),
          border: Border.all(
            color: _teal.withValues(alpha: 0.28 + glow * 0.1),
          ),
          boxShadow: [
            BoxShadow(
              color: _teal.withValues(alpha: 0.16 + glow * 0.16),
              blurRadius: 32 + glow * 18,
              spreadRadius: glow * 6,
            ),
          ],
        ),
        child: Icon(
          _phase == _ProvisioningPhase.success
              ? Icons.check_rounded
              : Icons.sensors_rounded,
          color: Colors.white.withValues(alpha: 0.9),
          size: 40,
        ),
      ),
    );
  }

  Widget _buildHeader() {
    final title = switch (_phase) {
      _ProvisioningPhase.scanning => 'Looking for your Rhythm Box',
      _ProvisioningPhase.devices => _devices.isEmpty
          ? (_hasAttemptedScan ? 'No devices found' : 'Set Up Your Rhythm Box')
          : 'Choose your Rhythm Box',
      _ProvisioningPhase.connecting => 'Connecting to $_selectedDeviceName',
      _ProvisioningPhase.credentials => 'Connect to Wi-Fi',
      _ProvisioningPhase.provisioning =>
        _provisionedIp == null ? 'Joining Wi-Fi' : 'Almost there',
      _ProvisioningPhase.success => 'You\'re all set',
      _ProvisioningPhase.error => 'Something went wrong',
    };

    final subtitle = switch (_phase) {
      _ProvisioningPhase.scanning =>
        'Searching for nearby Rhythm devices ready to be set up.',
      _ProvisioningPhase.devices => _devices.isEmpty
          ? (_hasAttemptedScan
              ? 'Make sure your Rhythm Box is powered on and nearby.'
              : 'Power on your Rhythm Box, keep it nearby, then start a Bluetooth scan.')
          : 'Tap the device you\'d like to set up.',
      _ProvisioningPhase.connecting =>
        'Reading device info and preparing for setup.',
      _ProvisioningPhase.credentials =>
        'Enter your Wi-Fi details so your Rhythm Box can join your network.',
      _ProvisioningPhase.provisioning => _provisionedIp == null
          ? 'Your device may show a pairing prompt — go ahead and accept it.'
          : 'Your Rhythm Box joined Wi-Fi at $_provisionedIp. Waiting for it to come online.',
      _ProvisioningPhase.success =>
        'Your Rhythm Box is online and ready to go.',
      _ProvisioningPhase.error => 'You can try again or go back.',
    };

    return Column(
      children: [
        Text(
          title,
          textAlign: TextAlign.center,
          style: const TextStyle(
            color: CelestialColors.textPrimary,
            fontSize: 24,
            fontWeight: FontWeight.w600,
          ),
        ),
        const SizedBox(height: 10),
        Text(
          subtitle,
          textAlign: TextAlign.center,
          style: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.82),
            fontSize: 14,
            height: 1.5,
          ),
        ),
      ],
    );
  }

  Widget _buildBody() {
    return switch (_phase) {
      _ProvisioningPhase.scanning => _buildScanningBody(),
      _ProvisioningPhase.devices => _buildDevicesBody(),
      _ProvisioningPhase.connecting => _buildProgressCard(
          'Pairing with $_selectedDeviceName...',
        ),
      _ProvisioningPhase.credentials => _buildCredentialsBody(),
      _ProvisioningPhase.provisioning => _buildProgressCard(
          _provisionedIp == null
              ? 'Connecting to your Wi-Fi network...'
              : 'Joined at $_provisionedIp — finishing setup...',
        ),
      _ProvisioningPhase.success => _buildSuccessBody(),
      _ProvisioningPhase.error => _buildErrorBody(),
    };
  }

  Widget _buildScanningBody() {
    return _buildProgressCard('Searching for nearby devices...');
  }

  Widget _buildDevicesBody() {
    if (_devices.isEmpty) {
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _buildInfoCard(
            icon: Icons.search_off_rounded,
            title: _hasAttemptedScan ? 'Nothing found yet' : 'Ready to scan',
            description: _hasAttemptedScan
                ? 'Make sure your Rhythm Box is powered on and nearby, then try again.'
                : 'Make sure your Rhythm Box is powered on and nearby, then start scanning.',
          ),
          const SizedBox(height: 16),
          _buildActionButton(
            onTap: _startScan,
            icon: _hasAttemptedScan
                ? Icons.refresh_rounded
                : Icons.bluetooth_searching_rounded,
            label: _hasAttemptedScan ? 'Scan Again' : 'Scan for Rhythm Box',
            isPrimary: true,
          ),
          const SizedBox(height: 12),
          _buildActionButton(
            onTap: _close,
            icon: Icons.arrow_back_rounded,
            label: 'Back',
            isPrimary: false,
          ),
        ],
      );
    }

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        for (final device in _devices) _buildDeviceCard(device),
        const SizedBox(height: 16),
        _buildActionButton(
          onTap: _startScan,
          icon: Icons.refresh_rounded,
          label: 'Scan Again',
          isPrimary: false,
        ),
      ],
    );
  }

  Widget _buildCredentialsBody() {
    final canSubmit = _ssidController.text.trim().isNotEmpty;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (_wifiErrorMessage != null) ...[
          _buildBanner(
            color: Colors.orange.shade400,
            icon: Icons.warning_amber_rounded,
            message: _wifiErrorMessage!,
          ),
          const SizedBox(height: 16),
        ],
        if (_deviceInfo != null) ...[
          _buildInfoCard(
            icon: Icons.memory_rounded,
            title: _deviceInfo!.name,
            description: [
              'Version ${_deviceInfo!.version}',
              if (_deviceInfo!.mac != null) _deviceInfo!.mac!,
            ].join(' • '),
          ),
          const SizedBox(height: 16),
        ],
        _buildTextField(
          controller: _ssidController,
          label: 'Wi-Fi network',
          hint: 'MyWifi',
          icon: Icons.wifi_rounded,
          obscureText: false,
        ),
        const SizedBox(height: 12),
        _buildTextField(
          controller: _passwordController,
          label: 'Password',
          hint: 'Password',
          icon: Icons.password_rounded,
          obscureText: _obscurePassword,
          trailing: GestureDetector(
            onTap: () {
              setState(() {
                _obscurePassword = !_obscurePassword;
              });
            },
            child: Icon(
              _obscurePassword
                  ? Icons.visibility_off_rounded
                  : Icons.visibility_rounded,
              color: CelestialColors.textSecondary.withValues(alpha: 0.7),
              size: 20,
            ),
          ),
        ),
        const SizedBox(height: 20),
        _buildActionButton(
          onTap: canSubmit ? _submitCredentials : null,
          icon: Icons.wifi_protected_setup_rounded,
          label: 'Connect',
          isPrimary: true,
        ),
        const SizedBox(height: 12),
        _buildActionButton(
          onTap: _startOver,
          icon: Icons.refresh_rounded,
          label: 'Choose Another Device',
          isPrimary: false,
        ),
      ],
    );
  }

  Widget _buildSuccessBody() {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _buildBanner(
          color: const Color(0xFF22C55E),
          icon: Icons.check_circle_rounded,
          message: _provisionedIp == null
              ? 'Setup complete — your Rhythm Box is ready.'
              : 'Setup complete at $_provisionedIp.',
        ),
        const SizedBox(height: 16),
        _buildActionButton(
          onTap: _close,
          icon: Icons.arrow_forward_rounded,
          label: 'Continue',
          isPrimary: true,
        ),
      ],
    );
  }

  Widget _buildErrorBody() {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _buildBanner(
          color: Colors.red.shade400,
          icon: Icons.error_outline_rounded,
          message: _errorMessage ?? 'An unexpected error occurred.',
        ),
        const SizedBox(height: 16),
        _buildActionButton(
          onTap: _startOver,
          icon: Icons.refresh_rounded,
          label: 'Try Again',
          isPrimary: true,
        ),
        const SizedBox(height: 12),
        _buildActionButton(
          onTap: _close,
          icon: Icons.arrow_back_rounded,
          label: 'Back',
          isPrimary: false,
        ),
      ],
    );
  }

  Widget _buildProgressCard(String label) {
    return Container(
      padding: const EdgeInsets.all(18),
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(18),
        border: Border.all(
          color: CelestialColors.orbitRing.withValues(alpha: 0.45),
        ),
      ),
      child: Column(
        children: [
          const SizedBox(
            width: 42,
            height: 42,
            child: CircularProgressIndicator(
              strokeWidth: 3,
              valueColor: AlwaysStoppedAnimation<Color>(_teal),
            ),
          ),
          const SizedBox(height: 14),
          Text(
            label,
            textAlign: TextAlign.center,
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.88),
              fontSize: 14,
              height: 1.5,
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildDeviceCard(BleDevice device) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 10),
      child: GestureDetector(
        onTap: () => _connectToDevice(device),
        child: Container(
          padding: const EdgeInsets.all(16),
          decoration: BoxDecoration(
            color: Colors.white.withValues(alpha: 0.05),
            borderRadius: BorderRadius.circular(16),
            border: Border.all(
              color: _teal.withValues(alpha: 0.18),
            ),
          ),
          child: Row(
            children: [
              Container(
                width: 42,
                height: 42,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: _teal.withValues(alpha: 0.16),
                ),
                child: const Icon(
                  Icons.developer_board_rounded,
                  color: _teal,
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
                    const SizedBox(height: 4),
                    Text(
                      'Signal ${_signalLabel(device.rssi)} • Ready to set up',
                      style: TextStyle(
                        color: CelestialColors.textSecondary
                            .withValues(alpha: 0.72),
                        fontSize: 12,
                      ),
                    ),
                  ],
                ),
              ),
              Icon(
                Icons.chevron_right_rounded,
                color: CelestialColors.textSecondary.withValues(alpha: 0.5),
              ),
            ],
          ),
        ),
      ),
    );
  }

  String _signalLabel(int rssi) {
    if (rssi >= -55) return 'strong';
    if (rssi >= -70) return 'good';
    if (rssi >= -82) return 'fair';
    return 'weak';
  }

  Widget _buildTextField({
    required TextEditingController controller,
    required String label,
    required String hint,
    required IconData icon,
    required bool obscureText,
    Widget? trailing,
  }) {
    return Container(
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(16),
        border: Border.all(
          color: CelestialColors.orbitRing.withValues(alpha: 0.45),
        ),
      ),
      child: TextField(
        controller: controller,
        obscureText: obscureText,
        style: const TextStyle(
          color: CelestialColors.textPrimary,
          fontSize: 15,
        ),
        decoration: InputDecoration(
          labelText: label,
          hintText: hint,
          hintStyle: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.45),
          ),
          labelStyle: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.75),
          ),
          prefixIcon: Icon(
            icon,
            color: _teal.withValues(alpha: 0.8),
          ),
          suffixIcon: trailing,
          border: InputBorder.none,
          contentPadding: const EdgeInsets.symmetric(
            horizontal: 16,
            vertical: 16,
          ),
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
        borderRadius: BorderRadius.circular(16),
        border: Border.all(
          color: CelestialColors.orbitRing.withValues(alpha: 0.45),
        ),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Container(
            width: 38,
            height: 38,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: _teal.withValues(alpha: 0.14),
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
                    color:
                        CelestialColors.textSecondary.withValues(alpha: 0.82),
                    fontSize: 13,
                    height: 1.45,
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildBanner({
    required Color color,
    required IconData icon,
    required String message,
  }) {
    return Container(
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.1),
        borderRadius: BorderRadius.circular(16),
        border: Border.all(
          color: color.withValues(alpha: 0.32),
        ),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(icon, color: color, size: 20),
          const SizedBox(width: 12),
          Expanded(
            child: Text(
              message,
              style: TextStyle(
                color: color,
                fontSize: 13,
                fontWeight: FontWeight.w500,
                height: 1.45,
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildActionButton({
    required VoidCallback? onTap,
    required IconData icon,
    required String label,
    required bool isPrimary,
  }) {
    final enabled = onTap != null;
    return GestureDetector(
      onTap: onTap,
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 180),
        padding: const EdgeInsets.symmetric(vertical: 16),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(16),
          gradient: isPrimary && enabled
              ? const LinearGradient(
                  begin: Alignment.topLeft,
                  end: Alignment.bottomRight,
                  colors: [_teal, _tealDeep],
                )
              : null,
          color: !isPrimary
              ? CelestialColors.backgroundCard
              : enabled
                  ? null
                  : CelestialColors.orbitRing.withValues(alpha: 0.28),
          border: isPrimary
              ? null
              : Border.all(
                  color: CelestialColors.orbitRing.withValues(alpha: 0.45),
                ),
          boxShadow: isPrimary && enabled
              ? [
                  BoxShadow(
                    color: _teal.withValues(alpha: 0.28),
                    blurRadius: 14,
                    offset: const Offset(0, 5),
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
                  ? Colors.white.withValues(alpha: enabled ? 1 : 0.5)
                  : CelestialColors.textPrimary
                      .withValues(alpha: enabled ? 1 : 0.5),
            ),
            const SizedBox(width: 10),
            Text(
              label,
              style: TextStyle(
                color: isPrimary
                    ? Colors.white.withValues(alpha: enabled ? 1 : 0.5)
                    : CelestialColors.textPrimary
                        .withValues(alpha: enabled ? 1 : 0.5),
                fontSize: 15,
                fontWeight: FontWeight.w600,
              ),
            ),
          ],
        ),
      ),
    );
  }
}
