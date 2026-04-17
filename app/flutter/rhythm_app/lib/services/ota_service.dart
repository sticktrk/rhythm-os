/// OTA update service for Rhythm devices.
///
/// Supports two flows:
/// - Legacy ESP32 binary upload via the SDK
/// - Capability-driven self-pull updates for rhythm-server / rpi builds
library;

import 'dart:async';

import 'package:dio/dio.dart';
import 'package:flutter/foundation.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' as sdk;

/// OTA update state machine surfaced to the settings UI.
enum OtaState {
  idle,
  checking,
  available,
  upToDate,
  downloading,
  uploading,
  flashing,
  rebooting,
  complete,
  error,
}

enum _OtaStrategy {
  unknown,
  unsupported,
  legacyUpload,
  selfPull,
}

/// Firmware release metadata from an update check.
class FirmwareRelease {
  final String version;
  final String url;
  final int? size;
  final String? changelog;

  const FirmwareRelease({
    required this.version,
    this.url = '',
    this.size,
    this.changelog,
  });

  factory FirmwareRelease._fromSdk(sdk.RhythmFirmwareRelease r) =>
      FirmwareRelease(
        version: r.version,
        url: r.url,
        size: r.size,
        changelog: r.changelog,
      );
}

class OtaCapabilities {
  final String strategy;
  final String scope;
  final bool canCheck;
  final bool canUpdate;
  final bool canUpload;
  final bool requiresRestart;
  final String rollback;

  const OtaCapabilities({
    required this.strategy,
    required this.scope,
    required this.canCheck,
    required this.canUpdate,
    required this.canUpload,
    required this.requiresRestart,
    required this.rollback,
  });

  factory OtaCapabilities.fromJson(Map<String, dynamic> json) {
    return OtaCapabilities(
      strategy: json['strategy']?.toString() ?? 'unknown',
      scope: json['scope']?.toString() ?? 'unknown',
      canCheck: json['can_check'] == true,
      canUpdate: json['can_update'] == true,
      canUpload: json['can_upload'] == true,
      requiresRestart: json['requires_restart'] == true,
      rollback: json['rollback']?.toString() ?? 'unknown',
    );
  }

  bool get supportsSelfPullBinaryUpdate =>
      strategy == 'self_pull' &&
      scope == 'binary' &&
      canCheck &&
      canUpdate &&
      !canUpload;
}

class _OtaStatusPayload {
  final String state;
  final String currentVersion;
  final String latestVersion;
  final String? targetVersion;
  final bool updateAvailable;
  final String? message;
  final String? lastError;

  const _OtaStatusPayload({
    required this.state,
    required this.currentVersion,
    required this.latestVersion,
    required this.targetVersion,
    required this.updateAvailable,
    required this.message,
    required this.lastError,
  });

  factory _OtaStatusPayload.fromJson(Map<String, dynamic> json) {
    return _OtaStatusPayload(
      state: json['state']?.toString() ?? 'idle',
      currentVersion: json['current_version']?.toString() ?? '0.0.0',
      latestVersion: json['latest_version']?.toString() ?? '0.0.0',
      targetVersion: json['target_version']?.toString(),
      updateAvailable: json['update_available'] == true,
      message: json['message']?.toString(),
      lastError: json['last_error']?.toString(),
    );
  }
}

/// ChangeNotifier wrapper for OTA state used by the settings UI.
class OtaService extends ChangeNotifier {
  final sdk.RhythmOtaApi _legacyApi = sdk.RhythmOtaApi();

  Dio? _dio;
  String? _host;
  int _port = 80;

  OtaState _state = OtaState.idle;
  _OtaStrategy _strategy = _OtaStrategy.unknown;
  OtaCapabilities? _capabilities;
  FirmwareRelease? _availableRelease;
  sdk.RhythmFirmwareRelease? _sdkRelease;
  int? _progress;
  String? _errorMessage;
  String? _statusMessage;
  String _currentVersion = '0.0.0';
  String? _latestVersion;
  String? _targetVersion;
  String? _preUpdateVersion;
  bool _isLoadingSupport = false;

  StreamSubscription<sdk.RhythmOtaProgress>? _updateSub;
  int _selfPullPollToken = 0;
  bool _disposed = false;

  OtaState get state => _state;
  FirmwareRelease? get availableRelease => _availableRelease;
  int? get progress => _progress;
  String? get errorMessage => _errorMessage;
  String? get statusMessage => _statusMessage;
  String get currentVersion => _currentVersion;
  String? get latestVersion => _latestVersion;
  OtaCapabilities? get capabilities => _capabilities;
  bool get isLoadingSupport => _isLoadingSupport;
  bool get showUpdateUi =>
      _strategy == _OtaStrategy.legacyUpload ||
      _strategy == _OtaStrategy.selfPull;
  bool get isSelfPull => _strategy == _OtaStrategy.selfPull;
  bool get isLegacyUpload => _strategy == _OtaStrategy.legacyUpload;
  bool get isProgressIndeterminate => _progress == null;

  Future<void> initialize({
    required String host,
    int port = 80,
    String? fallbackCurrentVersion,
    String? fallbackPlatformType,
    String? fallbackPlatformContext,
  }) async {
    _configureClient(host, port);

    final fallbackVersion = _normalizeVersion(fallbackCurrentVersion);
    if (fallbackVersion != null) {
      _currentVersion = fallbackVersion;
    }

    _isLoadingSupport = true;
    _errorMessage = null;
    notifyListeners();

    String? platformType = fallbackPlatformType;
    String? platformContext = fallbackPlatformContext;

    try {
      final stateJson = await _getJson('api/state');
      _currentVersion = _normalizeVersion(stateJson['version']?.toString()) ??
          _currentVersion;
      platformType = stateJson['platform']?.toString() ?? platformType;
      platformContext = stateJson['context']?.toString() ?? platformContext;
    } catch (_) {
      // Keep existing sync-provider state when /api/state is temporarily
      // unavailable during reconnects.
    }

    try {
      final capsJson = await _getJson('api/ota/capabilities');
      final capabilities = OtaCapabilities.fromJson(capsJson);
      _capabilities = capabilities;
      _strategy = capabilities.supportsSelfPullBinaryUpdate
          ? _OtaStrategy.selfPull
          : _OtaStrategy.unsupported;
    } catch (_) {
      _capabilities = null;
      _strategy = _looksLikeLegacyEmbedded(
        platformType: platformType,
        platformContext: platformContext,
      )
          ? _OtaStrategy.legacyUpload
          : _OtaStrategy.unsupported;
    }

    if (_strategy == _OtaStrategy.selfPull) {
      await _restoreSelfPullStatus();
    } else if (_strategy == _OtaStrategy.unsupported &&
        !_isTerminalState(_state)) {
      _state = OtaState.idle;
      _statusMessage = null;
      _availableRelease = null;
      _latestVersion = null;
      _targetVersion = null;
    }

    _isLoadingSupport = false;
    notifyListeners();
  }

  /// Check for an available update.
  Future<void> checkForUpdate(String currentVersion) async {
    final normalized = _normalizeVersion(currentVersion);
    if (normalized != null) {
      _currentVersion = normalized;
    }

    if (_strategy == _OtaStrategy.selfPull) {
      await _checkSelfPullUpdate();
      return;
    }

    if (_strategy != _OtaStrategy.legacyUpload) return;

    _state = OtaState.checking;
    _errorMessage = null;
    _statusMessage = null;
    notifyListeners();

    try {
      final release = await _legacyApi.checkForUpdate(_currentVersion);
      if (release != null) {
        _sdkRelease = release;
        _availableRelease = FirmwareRelease._fromSdk(release);
        _latestVersion = release.version;
        _targetVersion = release.version;
        _state = OtaState.available;
      } else {
        _sdkRelease = null;
        _availableRelease = null;
        _latestVersion = _currentVersion;
        _targetVersion = null;
        _state = OtaState.upToDate;
      }
    } catch (e) {
      _state = OtaState.error;
      _errorMessage = 'Failed to check for updates: $e';
    }

    notifyListeners();
  }

  /// Start the selected update flow.
  Future<void> startUpdate(String deviceIp, {int port = 80}) async {
    if (_strategy == _OtaStrategy.selfPull) {
      await _startSelfPullUpdate();
      return;
    }

    final release = _sdkRelease;
    if (release == null) return;

    _state = OtaState.downloading;
    _progress = 0;
    _errorMessage = null;
    _statusMessage = null;
    _preUpdateVersion = _currentVersion;
    notifyListeners();

    _updateSub?.cancel();
    _updateSub = _legacyApi
        .startUpdate(release, deviceHost: deviceIp, port: port)
        .listen(
      _onLegacyProgress,
      onError: (Object error) {
        _state = OtaState.error;
        _errorMessage = error.toString();
        _progress = null;
        notifyListeners();
      },
    );
  }

  Future<void> _checkSelfPullUpdate() async {
    _state = OtaState.checking;
    _errorMessage = null;
    _statusMessage = null;
    _progress = null;
    notifyListeners();

    try {
      final response = await _getJson('api/ota/check');
      final currentVersion =
          _normalizeVersion(response['current_version']?.toString()) ??
              _currentVersion;
      final latestVersion =
          _normalizeVersion(response['latest_version']?.toString()) ??
              currentVersion;
      final updateAvailable = response['update_available'] == true;

      _currentVersion = currentVersion;
      _latestVersion = latestVersion;
      _targetVersion = updateAvailable ? latestVersion : null;

      if (updateAvailable) {
        _availableRelease = FirmwareRelease(version: latestVersion);
        _state = OtaState.available;
      } else {
        _availableRelease = null;
        _state = OtaState.upToDate;
      }
    } catch (e) {
      _state = OtaState.error;
      _errorMessage = _formatError('Failed to check for updates', e);
    }

    notifyListeners();
  }

  Future<void> _startSelfPullUpdate() async {
    if (_strategy != _OtaStrategy.selfPull) return;

    _errorMessage = null;
    _statusMessage = 'Starting update...';
    _progress = null;
    _preUpdateVersion = _currentVersion;
    _state = OtaState.uploading;
    notifyListeners();

    try {
      final response = await _dio!.post('api/ota/update');
      if ((response.statusCode ?? 500) != 200) {
        throw StateError('Unexpected response: ${response.statusCode}');
      }
    } catch (e) {
      _state = OtaState.error;
      _errorMessage = _formatError('Failed to start update', e);
      notifyListeners();
      return;
    }

    unawaited(_pollSelfPullStatusUntilComplete());
  }

  Future<void> _restoreSelfPullStatus() async {
    try {
      final status = await _fetchSelfPullStatus();
      _applySelfPullStatus(status, allowIdleReset: true);
    } catch (_) {
      if (!_isUpdateInProgress(_state)) {
        _state = OtaState.idle;
        _statusMessage = null;
      }
    }
  }

  Future<void> _pollSelfPullStatusUntilComplete() async {
    final token = ++_selfPullPollToken;
    final deadline = DateTime.now().add(const Duration(minutes: 3));
    var expectingReconnect = false;

    while (!_disposed &&
        token == _selfPullPollToken &&
        DateTime.now().isBefore(deadline)) {
      try {
        final status = await _fetchSelfPullStatus();
        _applySelfPullStatus(status, allowIdleReset: false);

        if (status.state == 'error') {
          return;
        }

        if (_isUpdatedVersion(status.currentVersion)) {
          _markComplete(status.currentVersion);
          return;
        }

        if (_shouldInferCompletedRestart(
          status,
          expectingReconnect: expectingReconnect,
        )) {
          _markComplete(_completedVersionForStatus(status));
          return;
        }

        if (status.state == 'restarting') {
          expectingReconnect = true;
        }
      } catch (e) {
        if (_shouldTreatStatusPollErrorAsTransitional(e)) {
          expectingReconnect = true;
          _state = OtaState.rebooting;
          _statusMessage = 'Device restarting';
          _progress = null;
          notifyListeners();
        } else {
          _state = OtaState.error;
          _errorMessage = _formatError('Failed to poll update status', e);
          _statusMessage = null;
          notifyListeners();
          return;
        }
      }

      if (expectingReconnect) {
        final verified = await _verifyVersionFromState();
        if (verified) return;
      }

      await Future.delayed(const Duration(seconds: 2));
    }

    if (!_disposed &&
        token == _selfPullPollToken &&
        !_isTerminalState(_state)) {
      _state = OtaState.error;
      _errorMessage = 'Device did not come back online after update';
      _statusMessage = null;
      notifyListeners();
    }
  }

  Future<_OtaStatusPayload> _fetchSelfPullStatus() async {
    final json = await _getJson('api/ota/status');
    return _OtaStatusPayload.fromJson(json);
  }

  Future<bool> _verifyVersionFromState() async {
    try {
      final stateJson = await _getJson('api/state');
      final currentVersion =
          _normalizeVersion(stateJson['version']?.toString()) ??
              _currentVersion;
      _currentVersion = currentVersion;
      notifyListeners();

      if (_isUpdatedVersion(currentVersion)) {
        _markComplete(currentVersion);
        return true;
      }
    } catch (_) {
      // Device is still restarting.
    }

    return false;
  }

  void _applySelfPullStatus(
    _OtaStatusPayload status, {
    required bool allowIdleReset,
  }) {
    _currentVersion =
        _normalizeVersion(status.currentVersion) ?? _currentVersion;
    _latestVersion = _normalizeVersion(status.latestVersion) ?? _latestVersion;
    _targetVersion = _normalizeVersion(status.targetVersion) ?? _targetVersion;
    _availableRelease = status.updateAvailable && _latestVersion != null
        ? FirmwareRelease(version: _latestVersion!)
        : _availableRelease;
    _statusMessage = status.message;
    _progress = null;

    if (status.state == 'checking') {
      _state = OtaState.checking;
    } else if (status.state == 'ready') {
      if (status.updateAvailable && _latestVersion != null) {
        _availableRelease = FirmwareRelease(version: _latestVersion!);
      }
      _state = OtaState.available;
    } else if (status.state == 'updating') {
      _state = OtaState.uploading;
      _statusMessage ??= 'Installing update...';
    } else if (status.state == 'restarting') {
      _state = OtaState.rebooting;
      _statusMessage = 'Device restarting';
    } else if (status.state == 'error') {
      _state = OtaState.error;
      _errorMessage = status.lastError ?? status.message ?? 'Update failed';
    } else if (status.state == 'idle') {
      if (_isUpdatedVersion(status.currentVersion)) {
        _markComplete(status.currentVersion);
        return;
      }

      if (allowIdleReset) {
        if (status.updateAvailable && _latestVersion != null) {
          _availableRelease = FirmwareRelease(version: _latestVersion!);
          _state = OtaState.available;
        } else {
          _availableRelease = null;
          _state = OtaState.idle;
        }
      }
    }

    notifyListeners();
  }

  void _onLegacyProgress(sdk.RhythmOtaProgress event) {
    _state = _mapLegacyState(event.state);
    _progress = event.progressPercent;
    _errorMessage = event.errorMessage;
    _statusMessage = event.errorMessage;
    if (_state == OtaState.complete && _latestVersion != null) {
      _currentVersion = _latestVersion!;
    }
    notifyListeners();
  }

  OtaState _mapLegacyState(sdk.RhythmOtaState state) => switch (state) {
        sdk.RhythmOtaState.idle => OtaState.idle,
        sdk.RhythmOtaState.checking => OtaState.checking,
        sdk.RhythmOtaState.available => OtaState.available,
        sdk.RhythmOtaState.upToDate => OtaState.upToDate,
        sdk.RhythmOtaState.downloading => OtaState.downloading,
        sdk.RhythmOtaState.uploading => OtaState.uploading,
        sdk.RhythmOtaState.flashing => OtaState.flashing,
        sdk.RhythmOtaState.rebooting => OtaState.rebooting,
        sdk.RhythmOtaState.complete => OtaState.complete,
        sdk.RhythmOtaState.error => OtaState.error,
      };

  void _markComplete(String updatedVersion) {
    _currentVersion = _normalizeVersion(updatedVersion) ?? updatedVersion;
    _latestVersion = _currentVersion;
    _targetVersion = _currentVersion;
    _availableRelease = FirmwareRelease(version: _currentVersion);
    _statusMessage = 'Updated to v$_currentVersion';
    _errorMessage = null;
    _progress = 100;
    _state = OtaState.complete;
    notifyListeners();
  }

  void _configureClient(String host, int port) {
    if (_host == host && _port == port && _dio != null) return;

    _host = host;
    _port = port;
    _dio?.close();
    _dio = Dio(BaseOptions(
      baseUrl: port == 80 ? 'http://$host/' : 'http://$host:$port/',
      connectTimeout: const Duration(seconds: 5),
      receiveTimeout: const Duration(seconds: 10),
    ));
  }

  Future<Map<String, dynamic>> _getJson(String path) async {
    final response = await _dio!.get<Map<String, dynamic>>(path);
    final data = response.data;
    if (data == null) {
      throw StateError('Empty response from $path');
    }
    return Map<String, dynamic>.from(data);
  }

  bool _looksLikeLegacyEmbedded({
    String? platformType,
    String? platformContext,
  }) {
    return platformType == 'embedded' ||
        platformContext == 'embedded' ||
        platformContext == 'esp32';
  }

  bool _isUpdatedVersion(String version) {
    final normalized = _canonicalizeVersion(version);
    if (normalized == null) return false;

    final expected = _canonicalizeVersion(_targetVersion) ??
        _canonicalizeVersion(_latestVersion);
    if (expected != null && normalized == expected) {
      return true;
    }

    final previous = _canonicalizeVersion(_preUpdateVersion);
    return previous != null && normalized != previous;
  }

  bool _shouldInferCompletedRestart(
    _OtaStatusPayload status, {
    required bool expectingReconnect,
  }) {
    if (!expectingReconnect) return false;
    if (status.state != 'idle' && status.state != 'ready') return false;
    if (status.updateAvailable) return false;

    final lastError = status.lastError?.trim();
    if (lastError != null && lastError.isNotEmpty) return false;

    return true;
  }

  String _completedVersionForStatus(_OtaStatusPayload status) {
    final current = _normalizeVersion(status.currentVersion);
    if (_isUpdatedVersion(status.currentVersion) && current != null) {
      return current;
    }

    return _normalizeVersion(status.targetVersion) ??
        _normalizeVersion(status.latestVersion) ??
        current ??
        _currentVersion;
  }

  bool _isTerminalState(OtaState state) =>
      state == OtaState.complete || state == OtaState.error;

  bool _isUpdateInProgress(OtaState state) =>
      state == OtaState.downloading ||
      state == OtaState.uploading ||
      state == OtaState.flashing ||
      state == OtaState.rebooting;

  bool _shouldTreatStatusPollErrorAsTransitional(Object error) {
    if (_state != OtaState.uploading && _state != OtaState.rebooting) {
      return false;
    }

    if (error is DioException) {
      if (error.type == DioExceptionType.connectionTimeout ||
          error.type == DioExceptionType.receiveTimeout ||
          error.type == DioExceptionType.connectionError) {
        return true;
      }

      final dioMessage = [
        error.message,
        error.error?.toString(),
      ].whereType<String>().join(' ').toLowerCase();
      if (_looksLikeExpectedRestartDisconnect(dioMessage)) {
        return true;
      }
    }

    return _looksLikeExpectedRestartDisconnect(error.toString().toLowerCase());
  }

  bool _looksLikeExpectedRestartDisconnect(String message) {
    return message.contains('connection refused') ||
        message.contains('timed out') ||
        message.contains('timeout');
  }

  String _formatError(String prefix, Object error) {
    if (error is DioException) {
      final data = error.response?.data;
      if (data is Map<String, dynamic>) {
        final message = data['last_error']?.toString() ??
            data['message']?.toString() ??
            data['error']?.toString();
        if (message != null && message.isNotEmpty) {
          return '$prefix: $message';
        }
      }
    }
    return '$prefix: $error';
  }

  String? _normalizeVersion(String? version) {
    if (version == null) return null;
    final trimmed = version.trim();
    return trimmed.isEmpty ? null : trimmed;
  }

  String? _canonicalizeVersion(String? version) {
    final normalized = _normalizeVersion(version);
    if (normalized == null) return null;

    var canonical = normalized;
    if (canonical.startsWith('v') || canonical.startsWith('V')) {
      canonical = canonical.substring(1);
    }

    final metadataIndex = canonical.indexOf('+');
    if (metadataIndex >= 0) {
      canonical = canonical.substring(0, metadataIndex);
    }

    return canonical;
  }

  /// Reset the OTA UI back to idle.
  void reset() {
    _selfPullPollToken++;
    _updateSub?.cancel();
    _updateSub = null;
    _state = OtaState.idle;
    _availableRelease = null;
    _sdkRelease = null;
    _progress = null;
    _errorMessage = null;
    _statusMessage = null;
    _targetVersion = null;
    _preUpdateVersion = null;
    if (_strategy == _OtaStrategy.selfPull) {
      _latestVersion = _currentVersion;
    }
    notifyListeners();
  }

  @override
  void dispose() {
    _disposed = true;
    _selfPullPollToken++;
    _updateSub?.cancel();
    _dio?.close();
    super.dispose();
  }
}
