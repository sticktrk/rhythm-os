import 'dart:async';
import 'dart:io' show Platform;

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:permission_handler/permission_handler.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show RhythmAuthApi, RhythmConfigApi, RhythmDiagnosticsApi;

import '../../providers/home_provider.dart';
import '../../services/account_cloud_sync_service.dart';
import '../../services/analytics_service.dart';
import '../../services/ble_provisioning_service.dart';
import '../../services/recent_servers_service.dart';
import '../../widgets/solar_orbit.dart';
import 'add_home_flow.dart';

enum _ProvisioningPhase {
  scanning,
  devices,
  connecting,
  credentials,
  provisioning,
  updating,
  success,
  error,
}

class _HomeNameCancelledException implements Exception {
  const _HomeNameCancelledException();
}

class _KnownBleHome {
  const _KnownBleHome({
    required this.homeName,
  });

  final String homeName;
}

@visibleForTesting
bool bleProvisioningSupportsPlatformForTesting({
  required TargetPlatform platform,
  bool isWeb = false,
}) {
  if (isWeb) return false;
  return platform == TargetPlatform.android ||
      platform == TargetPlatform.iOS ||
      platform == TargetPlatform.macOS;
}

@visibleForTesting
bool bleProvisioningShowsUpdateStatusForTesting(
  ProvisioningStatusMessage status,
) {
  return status.status == 'updating' || status.status == 'restarting';
}

@visibleForTesting
String? bleProvisioningKnownHomeNameForTesting({
  required String deviceName,
  required Iterable<AccountHomeServerHubs> homeEntries,
}) {
  return _knownHomeForBleDeviceName(
    deviceName: deviceName,
    homeEntries: homeEntries,
  )?.homeName;
}

@visibleForTesting
Future<bool> bleProvisioningWaitForServerHealthForTesting({
  required Future<bool> Function() healthCheck,
  Duration timeout = _serverHealthTimeout,
  Duration pollInterval = _serverHealthPollInterval,
  Duration probeTimeout = _serverHealthProbeTimeout,
}) {
  return _waitForBleProvisioningServerHealth(
    healthCheck: healthCheck,
    timeout: timeout,
    pollInterval: pollInterval,
    probeTimeout: probeTimeout,
  );
}

@visibleForTesting
Future<bool> bleProvisioningWaitForServerRestartAndHealthForTesting({
  required Future<bool> Function() healthCheck,
  Duration offlineTimeout = _serverRestartOfflineTimeout,
  Duration onlineTimeout = _serverRestartOnlineTimeout,
  Duration pollInterval = _serverHealthPollInterval,
  Duration probeTimeout = _serverHealthProbeTimeout,
}) {
  return _waitForBleProvisioningServerRestartAndHealth(
    healthCheck: healthCheck,
    offlineTimeout: offlineTimeout,
    onlineTimeout: onlineTimeout,
    pollInterval: pollInterval,
    probeTimeout: probeTimeout,
  );
}

@visibleForTesting
Future<void> bleProvisioningAutoContinueForTesting({
  required VoidCallback showSuccess,
  required Future<void> Function() waitForSuccessFeedback,
  required bool Function() isMounted,
  required VoidCallback continueToConnectHub,
}) async {
  showSuccess();
  await waitForSuccessFeedback();
  if (isMounted()) continueToConnectHub();
}

@visibleForTesting
Future<String> bleProvisioningResolveVerifiedOwnerTokenForTesting({
  required Iterable<String?> candidates,
  required Future<bool> Function(String token) verifyToken,
  required Future<String> Function() claimToken,
}) async {
  final seen = <String>{};
  Object? lastVerificationError;
  for (final candidate in candidates) {
    final token = candidate?.trim();
    if (token == null || token.isEmpty || !seen.add(token)) continue;
    try {
      if (await verifyToken(token)) return token;
    } catch (error) {
      lastVerificationError = error;
    }
  }

  final claimed = (await claimToken()).trim();
  if (claimed.isEmpty) {
    throw StateError('Server returned an empty owner token.');
  }
  try {
    if (await verifyToken(claimed)) return claimed;
  } catch (error) {
    lastVerificationError = error;
  }

  final suffix = lastVerificationError == null
      ? ''
      : ' Last verification error: $lastVerificationError';
  throw StateError(
    'The owner token issued by the server could not be authenticated.$suffix',
  );
}

_KnownBleHome? _knownHomeForBleDeviceName({
  required String deviceName,
  required Iterable<AccountHomeServerHubs> homeEntries,
}) {
  final deviceKey = _bleDeviceMatchKey(deviceName);
  if (deviceKey == null) return null;

  for (final entry in homeEntries) {
    for (final hub in entry.serverHubs) {
      if (hub.type != HubType.server) continue;
      final hubKey = _bleDeviceMatchKey(hub.name);
      if (hubKey == deviceKey) {
        return _KnownBleHome(homeName: entry.home.name);
      }
    }
  }

  return null;
}

String? _bleDeviceMatchKey(String value) {
  final key = value.trim().toLowerCase().replaceAll(
        RegExp(r'[^a-z0-9]+'),
        '',
      );
  if (key.isEmpty || _genericBleDeviceNameKeys.contains(key)) return null;
  return key;
}

const _genericBleDeviceNameKeys = {
  'rhythm',
  'rhythmbox',
  'rhythmdevice',
  'rhythmserver',
};

const _serverHealthTimeout = Duration(seconds: 30);
const _serverRestartOfflineTimeout = Duration(seconds: 20);
const _serverRestartOnlineTimeout = Duration(minutes: 5);
const _serverHealthPollInterval = Duration(seconds: 1);
const _serverHealthProbeTimeout = Duration(seconds: 3);

Future<bool> _waitForBleProvisioningServerHealth({
  required Future<bool> Function() healthCheck,
  required Duration timeout,
  required Duration pollInterval,
  required Duration probeTimeout,
}) async {
  final deadline = DateTime.now().add(timeout);

  while (DateTime.now().isBefore(deadline)) {
    if (await _bleProvisioningServerIsHealthy(
      healthCheck,
      deadline: deadline,
      probeTimeout: probeTimeout,
    )) {
      return true;
    }
    await _delayUntilNextBleProvisioningProbe(
      deadline: deadline,
      pollInterval: pollInterval,
    );
  }
  return false;
}

Future<bool> _waitForBleProvisioningServerRestartAndHealth({
  required Future<bool> Function() healthCheck,
  required Duration offlineTimeout,
  required Duration onlineTimeout,
  required Duration pollInterval,
  required Duration probeTimeout,
}) async {
  final offlineDeadline = DateTime.now().add(offlineTimeout);
  var sawOffline = false;

  while (DateTime.now().isBefore(offlineDeadline)) {
    if (!await _bleProvisioningServerIsHealthy(
      healthCheck,
      deadline: offlineDeadline,
      probeTimeout: probeTimeout,
    )) {
      sawOffline = true;
      break;
    }
    await _delayUntilNextBleProvisioningProbe(
      deadline: offlineDeadline,
      pollInterval: pollInterval,
    );
  }

  if (!sawOffline) {
    return _bleProvisioningServerIsHealthy(
      healthCheck,
      deadline: DateTime.now().add(probeTimeout),
      probeTimeout: probeTimeout,
    );
  }

  return _waitForBleProvisioningServerHealth(
    healthCheck: healthCheck,
    timeout: onlineTimeout,
    pollInterval: pollInterval,
    probeTimeout: probeTimeout,
  );
}

Future<bool> _bleProvisioningServerIsHealthy(
  Future<bool> Function() healthCheck, {
  required DateTime deadline,
  required Duration probeTimeout,
}) async {
  final remaining = deadline.difference(DateTime.now());
  if (remaining <= Duration.zero) return false;
  final timeout = remaining < probeTimeout ? remaining : probeTimeout;
  try {
    return await healthCheck().timeout(timeout);
  } catch (_) {
    return false;
  }
}

Future<void> _delayUntilNextBleProvisioningProbe({
  required DateTime deadline,
  required Duration pollInterval,
}) async {
  final remaining = deadline.difference(DateTime.now());
  if (remaining <= Duration.zero) return;
  await Future<void>.delayed(
    remaining < pollInterval ? remaining : pollInterval,
  );
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
  String? _provisioningStatusMessage;
  String? _errorMessage;
  String? _wifiErrorMessage;
  String? _wifiScanErrorMessage;
  final List<WifiNetwork> _wifiNetworks = <WifiNetwork>[];
  String? _pendingPersistenceIp;
  String? _pendingPersistenceOwnerToken;
  bool _obscurePassword = true;
  bool _hasAttemptedScan = false;
  bool _wifiScanInProgress = false;
  bool _manualSsidEntry = false;
  bool _wifiScanUnsupported = false;

  bool get _supportsBleProvisioning =>
      bleProvisioningSupportsPlatformForTesting(
        platform: defaultTargetPlatform,
        isWeb: kIsWeb,
      );

  String get _selectedDeviceName =>
      _selectedDevice?.name ?? _deviceInfo?.name ?? 'Rhythm Box';

  String get _provisionedBoxName {
    final name = _deviceInfo?.name.trim();
    if (name == null || name.isEmpty || name == 'RhythmServer') {
      return 'Rhythm Box';
    }
    return name;
  }

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
        _wifiScanErrorMessage = null;
        _wifiNetworks.clear();
        _devices.clear();
        _selectedDevice = null;
        _deviceInfo = null;
        _provisionedIp = null;
        _provisioningStatusMessage = null;
        _pendingPersistenceIp = null;
        _pendingPersistenceOwnerToken = null;
        _hasAttemptedScan = false;
        _wifiScanInProgress = false;
        _manualSsidEntry = false;
        _wifiScanUnsupported = false;
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
            child: const Text('Open Settings', style: TextStyle(color: _teal)),
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
      _wifiScanErrorMessage = null;
      _wifiNetworks.clear();
      _devices.clear();
      _selectedDevice = null;
      _deviceInfo = null;
      _provisionedIp = null;
      _provisioningStatusMessage = null;
      _hasAttemptedScan = true;
      _wifiScanInProgress = false;
      _manualSsidEntry = false;
      _wifiScanUnsupported = false;
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
      });
      final status = info.provisioningStatus;
      if (status != null &&
          status.status != 'waiting' &&
          status.status != 'wifi_failed' &&
          status.canResume) {
        await _resumeProvisioning(status);
        return;
      }

      if (!mounted) return;
      setState(() {
        _phase = _ProvisioningPhase.credentials;
        _wifiScanErrorMessage = null;
        _wifiNetworks.clear();
        _wifiScanInProgress = false;
        _manualSsidEntry = false;
        _wifiScanUnsupported = !_bleService.supportsWifiScan;
      });
      if (_bleService.supportsWifiScan) {
        unawaited(_scanWifiNetworks());
      }
    } catch (error) {
      if (!mounted) return;
      setState(() {
        _phase = _ProvisioningPhase.error;
        _errorMessage = 'Couldn\'t connect to ${device.name}. $error';
      });
    }
  }

  Future<void> _scanWifiNetworks() async {
    if (!_bleService.supportsWifiScan) {
      if (!mounted) return;
      setState(() {
        _wifiScanUnsupported = true;
        _wifiScanInProgress = false;
        _manualSsidEntry = true;
      });
      return;
    }

    setState(() {
      _wifiScanInProgress = true;
      _wifiScanErrorMessage = null;
      _wifiNetworks.clear();
      _manualSsidEntry = false;
    });

    try {
      AnalyticsService().logEvent('ble_wifi_scan_started');
      final networks = await _bleService.scanWifiNetworks();
      if (!mounted) return;
      setState(() {
        _wifiNetworks
          ..clear()
          ..addAll(networks);
        _wifiScanInProgress = false;
        _manualSsidEntry = networks.isEmpty;
        _wifiScanErrorMessage =
            networks.isEmpty ? 'No Wi-Fi networks found.' : null;
      });
    } on UnsupportedError {
      if (!mounted) return;
      setState(() {
        _wifiScanUnsupported = true;
        _wifiScanInProgress = false;
        _manualSsidEntry = true;
      });
    } catch (error) {
      if (!mounted) return;
      AnalyticsService().logEvent('ble_wifi_scan_failed');
      setState(() {
        _wifiScanInProgress = false;
        _manualSsidEntry = true;
        _wifiScanErrorMessage =
            'Wi-Fi scan failed. Enter the network manually.';
      });
    }
  }

  void _selectWifiNetwork(String? ssid) {
    final selected = ssid?.trim();
    if (selected == null || selected.isEmpty) return;
    HapticFeedback.selectionClick();
    _ssidController.text = selected;
    if (_wifiNetworkForSsid(selected)?.security == 'open') {
      _passwordController.clear();
    }
    setState(() {
      _manualSsidEntry = false;
      _wifiErrorMessage = null;
    });
  }

  void _showManualSsidEntry() {
    HapticFeedback.selectionClick();
    setState(() {
      _manualSsidEntry = true;
      _wifiScanErrorMessage = null;
    });
  }

  Future<void> _submitCredentials() async {
    final ssid = _ssidController.text.trim();
    final selectedNetwork = _manualSsidEntry ? null : _wifiNetworkForSsid(ssid);
    final password =
        selectedNetwork?.security == 'open' ? '' : _passwordController.text;
    if (ssid.isEmpty) return;

    AnalyticsService().logEvent('ble_provisioning_started');
    HapticFeedback.mediumImpact();

    setState(() {
      _phase = _ProvisioningPhase.provisioning;
      _errorMessage = null;
      _wifiErrorMessage = null;
      _provisionedIp = null;
      _provisioningStatusMessage = null;
    });

    try {
      final result = await _bleService.sendWifiCredentials(
        ssid,
        password,
        onStatus: _handleProvisioningStatus,
      );
      await _finishProvisioning(result);
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

  Future<void> _resumeProvisioning(ProvisioningStatusMessage status) async {
    AnalyticsService().logEvent('ble_provisioning_resumed', {
      'status': status.status,
      if (status.otaStage != null) 'ota_stage': status.otaStage!,
    });
    _handleProvisioningStatus(status);

    try {
      final result = status.isTerminal
          ? BleProvisioningService.resultFromTerminalStatus(status)
          : await _bleService.waitForProvisioningResult(
              onStatus: _handleProvisioningStatus,
            );
      await _finishProvisioning(result);
    } catch (error) {
      if (!mounted) return;
      AnalyticsService().logEvent('ble_provisioning_failed', {
        'stage': 'resume',
      });
      setState(() {
        _phase = _ProvisioningPhase.error;
        _errorMessage = 'Setup failed: $error';
      });
    }
  }

  Future<void> _finishProvisioning(BleProvisioningResult result) async {
    if (!mounted) return;

    var completedResult = result;
    if (result.updateInProgress) {
      ProvisioningStatusMessage? completedStatus;
      try {
        completedStatus = await _bleService.waitForPostHandoffUpdate(
          onStatus: _handleProvisioningStatus,
        );
      } catch (error) {
        debugPrint(
          '[BLE] update progress handoff ended before a terminal status: '
          '$error',
        );
        completedResult = BleProvisioningResult(
          ip: result.ip,
          ownerToken: result.ownerToken,
          restartPending: true,
        );
      }
      if (completedStatus != null) {
        completedResult =
            BleProvisioningService.resultFromTerminalStatus(completedStatus);
      }
    }
    if (!mounted) return;

    final ip = completedResult.ip;

    setState(() {
      _provisionedIp = ip;
    });

    final online = completedResult.restartPending
        ? await _waitForServerRestartAndHealth(ip)
        : await _waitForServerHealth(ip);
    if (!mounted) return;
    if (!online) {
      AnalyticsService().logEvent('ble_provisioning_failed', {
        'stage': completedResult.restartPending
            ? 'post_update_health_timeout'
            : 'health_timeout',
      });
      setState(() {
        _phase = _ProvisioningPhase.error;
        _errorMessage = completedResult.restartPending
            ? 'Your Rhythm Box installed an update but did not come back online in time. Make sure you\'re on the same network and try again.'
            : 'Your Rhythm Box joined Wi-Fi at $ip but didn\'t come online in time. Make sure you\'re on the same network and try again.';
      });
      return;
    }

    final ownerToken = await _resolveOwnerTokenAfterProvisioning(
      ip,
      completedResult.ownerToken ?? result.ownerToken,
    );
    if (!mounted) return;

    _pendingPersistenceIp = ip;
    _pendingPersistenceOwnerToken = ownerToken;
    await _completePendingPersistence();
  }

  Future<void> _completePendingPersistence() async {
    final ip = _pendingPersistenceIp;
    if (ip == null || ip.isEmpty || !mounted) return;
    final ownerToken = _pendingPersistenceOwnerToken;

    setState(() {
      _phase = _ProvisioningPhase.provisioning;
      _provisionedIp = ip;
      _errorMessage = null;
    });

    try {
      await _persistServerHub(ip, ownerToken);
    } catch (error) {
      if (!mounted) return;
      AnalyticsService().logEvent('ble_provisioning_failed', {
        'stage': 'home_persistence',
      });
      setState(() {
        _phase = _ProvisioningPhase.error;
        _errorMessage = error is _HomeNameCancelledException
            ? 'Your Rhythm Box is online at $ip. Name the Home to finish setup.'
            : 'Your Rhythm Box is online at $ip, but the Home could not be saved. Try finishing setup again. ($error)';
      });
      return;
    }
    if (!mounted) return;

    _pendingPersistenceIp = null;
    _pendingPersistenceOwnerToken = null;
    AnalyticsService().logEvent('ble_provisioning_completed');
    HapticFeedback.heavyImpact();
    await bleProvisioningAutoContinueForTesting(
      showSuccess: () {
        setState(() {
          _phase = _ProvisioningPhase.success;
        });
      },
      waitForSuccessFeedback: () => Future<void>.delayed(
        const Duration(milliseconds: 750),
      ),
      isMounted: () => mounted,
      continueToConnectHub: _close,
    );
  }

  Future<bool> _waitForServerHealth(String ip) async {
    final client = RhythmDiagnosticsApi(host: ip, port: 54448);
    return _waitForBleProvisioningServerHealth(
      healthCheck: client.healthCheck,
      timeout: _serverHealthTimeout,
      pollInterval: _serverHealthPollInterval,
      probeTimeout: _serverHealthProbeTimeout,
    );
  }

  Future<bool> _waitForServerRestartAndHealth(String ip) async {
    final client = RhythmDiagnosticsApi(host: ip, port: 54448);
    return _waitForBleProvisioningServerRestartAndHealth(
      healthCheck: client.healthCheck,
      offlineTimeout: _serverRestartOfflineTimeout,
      onlineTimeout: _serverRestartOnlineTimeout,
      pollInterval: _serverHealthPollInterval,
      probeTimeout: _serverHealthProbeTimeout,
    );
  }

  void _handleProvisioningStatus(ProvisioningStatusMessage status) {
    if (!mounted) return;
    if (status.ip != null && status.ip!.isNotEmpty) {
      _provisionedIp = status.ip;
    }
    if (bleProvisioningShowsUpdateStatusForTesting(status)) {
      final message = status.message?.trim();
      setState(() {
        _phase = _ProvisioningPhase.updating;
        _provisioningStatusMessage =
            message == null || message.isEmpty ? null : message;
      });
    } else if (status.isConnected) {
      setState(() {
        _phase = _ProvisioningPhase.provisioning;
        _provisioningStatusMessage = null;
      });
    } else if (status.status == 'connecting') {
      setState(() {
        _phase = _ProvisioningPhase.provisioning;
        _provisioningStatusMessage = null;
      });
    }
  }

  Future<String?> _resolveOwnerTokenAfterProvisioning(
    String ip,
    String? provisionedOwnerToken,
  ) async {
    final storedTokens = _storedOwnerTokensForProvisionedServer(ip);
    final deadline = DateTime.now().add(const Duration(seconds: 12));
    Object? lastLanAuthError;

    while (true) {
      try {
        return await bleProvisioningResolveVerifiedOwnerTokenForTesting(
          candidates: [provisionedOwnerToken, ...storedTokens],
          verifyToken: (token) async {
            final status = await RhythmAuthApi(
              baseUrl: 'http://$ip:54448',
              authToken: token,
            ).getStatus().timeout(const Duration(seconds: 3));
            // Older appliance builds do not report the authenticated role.
            // Preserve their setup behavior until the firmware half of this
            // contract is installed; new builds can positively verify owner.
            return !status.reportsAuthenticatedRole ||
                status.hasAuthenticatedOwner;
          },
          claimToken: () async {
            final authApi = RhythmAuthApi(baseUrl: 'http://$ip:54448');
            final status =
                await authApi.getStatus().timeout(const Duration(seconds: 3));
            if (!status.claimAvailable) {
              throw StateError(
                'Server is already owner-configured and local claiming is unavailable.',
              );
            }
            final claim = await authApi
                .claimOwnerToken()
                .timeout(const Duration(seconds: 4));
            return claim.token;
          },
        );
      } catch (error) {
        lastLanAuthError = error;
        debugPrint('[BLE] owner token verification over LAN failed: $error');
      }

      if (!DateTime.now().isBefore(deadline)) {
        break;
      }
      await Future<void>.delayed(const Duration(seconds: 1));
    }

    throw StateError(
      'Server requires API auth, but owner token was not available over Wi-Fi.'
      ' Last error: $lastLanAuthError',
    );
  }

  List<String> _storedOwnerTokensForProvisionedServer(String ip) {
    final exactTokens = <String>[];
    final fallbackTokens = <String>[];

    void addToken(String? token, {required bool exact}) {
      final clean = token?.trim();
      if (clean == null || clean.isEmpty) return;
      if (exactTokens.contains(clean) || fallbackTokens.contains(clean)) {
        return;
      }
      (exact ? exactTokens : fallbackTokens).add(clean);
    }

    final homeProvider = context.read<HomeProvider>();
    final savedHubs = <Hub>[
      for (final snapshot in homeProvider.homeServerHubSnapshots)
        ...snapshot.serverHubs,
      ...homeProvider.currentHomeHubs.where(
        (hub) => hub.type == HubType.server,
      ),
    ];
    for (final hub in savedHubs) {
      if (hub.type != HubType.server) continue;
      addToken(
        hub.token,
        exact: hub.endpoint.host == ip && hub.endpoint.port == 54448,
      );
    }

    for (final recent in RecentServersService.instance.servers) {
      addToken(
        recent.token,
        exact: recent.host == ip && recent.port == 54448,
      );
    }

    return [...exactTokens, ...fallbackTokens];
  }

  Future<Hub> _persistServerHub(String ip, String? ownerToken) async {
    final homeProvider = context.read<HomeProvider>();
    final displayName = _provisionedBoxName;
    final serverInstanceId = await _serverInstanceIdFor(
      ip: ip,
      ownerToken: ownerToken,
    );
    final existingHome = _homeEntryForServerEndpoint(
      homeProvider: homeProvider,
      ip: ip,
      ownerToken: ownerToken,
      serverInstanceId: serverInstanceId,
    );
    if (existingHome != null) {
      final hub = await homeProvider.enterHome(existingHome);
      if (hub == null) {
        throw StateError('The saved Home could not be opened.');
      }
      await RecentServersService.instance.record(
        name: hub.name,
        host: ip,
        port: 54448,
        token: ownerToken ?? hub.token,
        serverInstanceId: serverInstanceId ?? hub.serverInstanceId,
      );
      return hub;
    }

    if (!mounted) throw const _HomeNameCancelledException();
    final home = await AddHomeFlow.show(context, defaultName: displayName);
    if (home == null) throw const _HomeNameCancelledException();

    final hub = await homeProvider.addServerHubInNewHome(
      homeName: home.name,
      hubName: displayName,
      host: ip,
      port: 54448,
      token: ownerToken,
      serverInstanceId: serverInstanceId,
      location: home.location,
      timezone: home.timezone,
    );
    if (hub == null) {
      throw StateError('The new Home could not be saved.');
    }
    await RecentServersService.instance.record(
      name: hub.name,
      host: ip,
      port: 54448,
      token: ownerToken ?? hub.token,
      serverInstanceId: serverInstanceId ?? hub.serverInstanceId,
    );
    return hub;
  }

  Future<String?> _serverInstanceIdFor({
    required String ip,
    required String? ownerToken,
  }) async {
    try {
      final state = await RhythmConfigApi(
        baseUrl: 'http://$ip:54448/',
        authToken: ownerToken,
      ).getState().timeout(const Duration(seconds: 4));
      final serverInstanceId = state.serverInstanceId?.trim();
      return serverInstanceId != null && serverInstanceId.isNotEmpty
          ? serverInstanceId
          : null;
    } catch (error) {
      debugPrint(
        '[BLE] server identity unavailable after provisioning: $error',
      );
      return null;
    }
  }

  AccountHomeServerHubs? _homeEntryForServerEndpoint({
    required HomeProvider homeProvider,
    required String ip,
    required String? ownerToken,
    required String? serverInstanceId,
  }) {
    final cleanServerInstanceId = serverInstanceId?.trim();
    final cleanOwnerToken = ownerToken?.trim();
    for (final snapshot in homeProvider.homeServerHubSnapshots) {
      final nextHubs = <Hub>[];
      var matched = false;
      for (final hub in snapshot.serverHubs) {
        final hubServerInstanceId = hub.serverInstanceId?.trim();
        final bothHaveServerIdentity = cleanServerInstanceId != null &&
            cleanServerInstanceId.isNotEmpty &&
            hubServerInstanceId != null &&
            hubServerInstanceId.isNotEmpty;
        var matches = hub.type == HubType.server &&
            bothHaveServerIdentity &&
            hubServerInstanceId == cleanServerInstanceId;
        if (!matches && !bothHaveServerIdentity) {
          matches = hub.type == HubType.server &&
              hub.endpoint.host == ip &&
              hub.endpoint.port == 54448;
          final hubToken = hub.token?.trim();
          if (matches &&
              cleanOwnerToken != null &&
              cleanOwnerToken.isNotEmpty &&
              hubToken != null &&
              hubToken.isNotEmpty &&
              cleanOwnerToken != hubToken) {
            matches = false;
          }
        }
        if (!matches) {
          nextHubs.add(hub);
          continue;
        }

        matched = true;
        final hasSavedToken = hub.token?.trim().isNotEmpty == true;
        nextHubs.add(
          hub.copyWith(
            token: cleanOwnerToken != null &&
                    cleanOwnerToken.isNotEmpty &&
                    !hasSavedToken
                ? cleanOwnerToken
                : hub.token,
            serverInstanceId: cleanServerInstanceId,
            updatedAt: DateTime.now(),
            pendingSync: true,
          ),
        );
      }

      if (matched) {
        return AccountHomeServerHubs(
          home: snapshot.home,
          serverHubs: List.unmodifiable(nextHubs),
        );
      }
    }
    return null;
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
      _wifiScanErrorMessage = null;
      _wifiNetworks.clear();
      _devices.clear();
      _selectedDevice = null;
      _deviceInfo = null;
      _provisionedIp = null;
      _provisioningStatusMessage = null;
      _pendingPersistenceIp = null;
      _pendingPersistenceOwnerToken = null;
      _hasAttemptedScan = false;
      _wifiScanInProgress = false;
      _manualSsidEntry = false;
      _wifiScanUnsupported = false;
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
          border: Border.all(color: _teal.withValues(alpha: 0.28 + glow * 0.1)),
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
      _ProvisioningPhase.updating => 'Updating your Rhythm Box',
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
      _ProvisioningPhase.updating =>
        'Installing the latest stable update so your box is ready to go.',
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
      _ProvisioningPhase.updating => _buildProgressCard(
          _provisioningStatusMessage ?? 'Updating to the latest version...',
        ),
      _ProvisioningPhase.success => _buildSuccessBody(),
      _ProvisioningPhase.error => _buildErrorBody(),
    };
  }

  Widget _buildScanningBody() {
    return _buildProgressCard('Searching for nearby devices...');
  }

  Widget _buildDevicesBody() {
    final homeEntries =
        context.watch<HomeProvider?>()?.homeServerHubSnapshots ??
            const <AccountHomeServerHubs>[];
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
        for (final device in _devices) _buildDeviceCard(device, homeEntries),
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
    final selectedNetwork = _manualSsidEntry
        ? null
        : _wifiNetworkForSsid(_ssidController.text.trim());
    final isOpenNetwork = selectedNetwork?.security == 'open';
    final canSubmit = _ssidController.text.trim().isNotEmpty &&
        (selectedNetwork == null ||
            isOpenNetwork ||
            _passwordController.text.isNotEmpty);
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
        _buildWifiNetworkField(),
        const SizedBox(height: 12),
        if (isOpenNetwork)
          _buildInfoCard(
            icon: Icons.lock_open_rounded,
            title: 'Open network',
            description: 'No password is required for this network.',
          )
        else
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
    final canFinishPersistence = _pendingPersistenceIp != null;
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
          onTap:
              canFinishPersistence ? _completePendingPersistence : _startOver,
          icon:
              canFinishPersistence ? Icons.home_rounded : Icons.refresh_rounded,
          label: canFinishPersistence ? 'Finish Setup' : 'Try Again',
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

  Widget _buildDeviceCard(
    BleDevice device,
    Iterable<AccountHomeServerHubs> homeEntries,
  ) {
    final knownHome = _knownHomeForBleDeviceName(
      deviceName: device.name,
      homeEntries: homeEntries,
    );

    return Padding(
      padding: const EdgeInsets.only(bottom: 10),
      child: GestureDetector(
        onTap: () => _connectToDevice(device),
        child: Container(
          padding: const EdgeInsets.all(16),
          decoration: BoxDecoration(
            color: Colors.white.withValues(alpha: 0.05),
            borderRadius: BorderRadius.circular(16),
            border: Border.all(color: _teal.withValues(alpha: 0.18)),
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
                child: const Icon(Icons.developer_board_rounded, color: _teal),
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
                      knownHome == null
                          ? 'Signal ${_signalLabel(device.rssi)} • Ready to set up'
                          : 'Signal ${_signalLabel(device.rssi)} • Already in ${knownHome.homeName}',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        color: knownHome == null
                            ? CelestialColors.textSecondary
                                .withValues(alpha: 0.72)
                            : _teal.withValues(alpha: 0.88),
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

  WifiNetwork? _wifiNetworkForSsid(String ssid) {
    for (final network in _wifiNetworks) {
      if (network.ssid == ssid) return network;
    }
    return null;
  }

  Widget _buildWifiNetworkField() {
    final selectedSsid = _ssidController.text.trim();
    final selectedValue =
        _wifiNetworks.any((network) => network.ssid == selectedSsid)
            ? selectedSsid
            : null;

    if (_wifiScanUnsupported || _manualSsidEntry || _wifiNetworks.isEmpty) {
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (_wifiScanInProgress) ...[
            _buildInfoCard(
              icon: Icons.wifi_find_rounded,
              title: 'Scanning Wi-Fi',
              description: 'Looking for nearby networks.',
            ),
            const SizedBox(height: 12),
          ],
          if (_wifiScanErrorMessage != null) ...[
            _buildBanner(
              color: Colors.orange.shade400,
              icon: Icons.warning_amber_rounded,
              message: _wifiScanErrorMessage!,
            ),
            const SizedBox(height: 12),
          ],
          _buildTextField(
            controller: _ssidController,
            label: 'Wi-Fi network',
            hint: 'MyWifi',
            icon: Icons.wifi_rounded,
            obscureText: false,
          ),
          if (!_wifiScanUnsupported && !_wifiScanInProgress) ...[
            const SizedBox(height: 10),
            _buildInlineAction(
              icon: Icons.refresh_rounded,
              label: 'Scan networks',
              onTap: _scanWifiNetworks,
            ),
          ],
        ],
      );
    }

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Container(
          decoration: BoxDecoration(
            color: CelestialColors.backgroundCard,
            borderRadius: BorderRadius.circular(16),
            border: Border.all(
              color: CelestialColors.orbitRing.withValues(alpha: 0.45),
            ),
          ),
          child: DropdownButtonFormField<String>(
            initialValue: selectedValue,
            isExpanded: true,
            dropdownColor: CelestialColors.backgroundCard,
            iconEnabledColor: CelestialColors.textSecondary.withValues(
              alpha: 0.7,
            ),
            style: const TextStyle(
              color: CelestialColors.textPrimary,
              fontSize: 15,
            ),
            decoration: InputDecoration(
              labelText: 'Wi-Fi network',
              labelStyle: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.75),
              ),
              prefixIcon: Icon(
                Icons.wifi_rounded,
                color: _teal.withValues(alpha: 0.8),
              ),
              border: InputBorder.none,
              contentPadding: const EdgeInsets.symmetric(
                horizontal: 16,
                vertical: 14,
              ),
            ),
            items: [
              for (final network in _wifiNetworks)
                DropdownMenuItem<String>(
                  value: network.ssid,
                  child: _WifiNetworkMenuItem(network: network),
                ),
            ],
            onChanged: _selectWifiNetwork,
          ),
        ),
        const SizedBox(height: 10),
        Row(
          children: [
            Expanded(
              child: _buildInlineAction(
                icon: _wifiScanInProgress
                    ? Icons.hourglass_top_rounded
                    : Icons.refresh_rounded,
                label: _wifiScanInProgress ? 'Scanning' : 'Refresh',
                onTap: _wifiScanInProgress ? null : _scanWifiNetworks,
              ),
            ),
            const SizedBox(width: 10),
            Expanded(
              child: _buildInlineAction(
                icon: Icons.edit_rounded,
                label: 'Manual',
                onTap: _showManualSsidEntry,
              ),
            ),
          ],
        ),
      ],
    );
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
          prefixIcon: Icon(icon, color: _teal.withValues(alpha: 0.8)),
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
            child: Icon(icon, color: _teal, size: 18),
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
                    color: CelestialColors.textSecondary.withValues(
                      alpha: 0.82,
                    ),
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
        border: Border.all(color: color.withValues(alpha: 0.32)),
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

  Widget _buildInlineAction({
    required IconData icon,
    required String label,
    required VoidCallback? onTap,
  }) {
    final enabled = onTap != null;
    return GestureDetector(
      onTap: onTap,
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 180),
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
        decoration: BoxDecoration(
          color: enabled
              ? Colors.white.withValues(alpha: 0.05)
              : Colors.white.withValues(alpha: 0.025),
          borderRadius: BorderRadius.circular(12),
          border: Border.all(
            color: CelestialColors.orbitRing.withValues(alpha: 0.38),
          ),
        ),
        child: Row(
          mainAxisAlignment: MainAxisAlignment.center,
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(
              icon,
              color: enabled
                  ? _teal.withValues(alpha: 0.82)
                  : CelestialColors.textSecondary.withValues(alpha: 0.45),
              size: 17,
            ),
            const SizedBox(width: 8),
            Flexible(
              child: Text(
                label,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  color: enabled
                      ? CelestialColors.textPrimary.withValues(alpha: 0.86)
                      : CelestialColors.textSecondary.withValues(alpha: 0.5),
                  fontSize: 13,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
          ],
        ),
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
                  : CelestialColors.textPrimary.withValues(
                      alpha: enabled ? 1 : 0.5,
                    ),
            ),
            const SizedBox(width: 10),
            Text(
              label,
              style: TextStyle(
                color: isPrimary
                    ? Colors.white.withValues(alpha: enabled ? 1 : 0.5)
                    : CelestialColors.textPrimary.withValues(
                        alpha: enabled ? 1 : 0.5,
                      ),
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

class _WifiNetworkMenuItem extends StatelessWidget {
  const _WifiNetworkMenuItem({required this.network});

  final WifiNetwork network;

  @override
  Widget build(BuildContext context) {
    final details = [
      if (network.rssi != null) _signalLabel(network.rssi!),
      if (network.security?.isNotEmpty == true) network.security!.toUpperCase(),
      if (network.band.isNotEmpty) network.band,
    ].join(' • ');

    return Row(
      children: [
        Icon(
          _signalIcon(network.rssi),
          color: const Color(0xFF00BCD4).withValues(alpha: 0.82),
          size: 18,
        ),
        const SizedBox(width: 10),
        Expanded(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                network.ssid,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: const TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 14,
                  fontWeight: FontWeight.w600,
                ),
              ),
              if (details.isNotEmpty) ...[
                const SizedBox(height: 2),
                Text(
                  details,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: CelestialColors.textSecondary.withValues(
                      alpha: 0.72,
                    ),
                    fontSize: 11,
                  ),
                ),
              ],
            ],
          ),
        ),
      ],
    );
  }

  static String _signalLabel(int rssi) {
    if (rssi >= -55) return 'strong';
    if (rssi >= -70) return 'good';
    if (rssi >= -82) return 'fair';
    return 'weak';
  }

  static IconData _signalIcon(int? rssi) {
    if (rssi == null) return Icons.wifi_rounded;
    if (rssi >= -55) return Icons.signal_wifi_4_bar_rounded;
    if (rssi >= -70) return Icons.network_wifi_3_bar_rounded;
    if (rssi >= -82) return Icons.network_wifi_2_bar_rounded;
    return Icons.network_wifi_1_bar_rounded;
  }
}
