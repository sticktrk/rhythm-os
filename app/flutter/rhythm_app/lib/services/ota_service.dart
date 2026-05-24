/// OTA update service for Rhythm devices.
///
/// Supports two flows:
/// - Legacy Rhythm bridge binary upload via the SDK
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

enum OtaUpdateReason {
  versionMismatch,
  componentDrift,
  unknown,
}

OtaUpdateReason? _tryParseOtaUpdateReason(Object? value) {
  final raw = value?.toString().trim();
  if (raw == null || raw.isEmpty) return null;

  return switch (raw) {
    'version_mismatch' => OtaUpdateReason.versionMismatch,
    'component_drift' => OtaUpdateReason.componentDrift,
    _ => OtaUpdateReason.unknown,
  };
}

bool? _tryParseBool(Object? value) {
  if (value is bool) return value;

  final raw = value?.toString().trim().toLowerCase();
  if (raw == null || raw.isEmpty) return null;
  if (raw == 'true' || raw == '1' || raw == 'yes') return true;
  if (raw == 'false' || raw == '0' || raw == 'no') return false;
  return null;
}

String? _normalizeOtaVersion(String? version) {
  if (version == null) return null;
  final trimmed = version.trim();
  return trimmed.isEmpty ? null : trimmed;
}

String? _canonicalizeOtaVersion(String? version) {
  final normalized = _normalizeOtaVersion(version);
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

bool _inferUpdateAvailable({
  required Object? updateAvailableValue,
  required String? currentVersion,
  required String? latestVersion,
  required OtaUpdateReason? updateReason,
}) {
  if (updateAvailableValue == true) {
    return true;
  }

  if (updateReason == OtaUpdateReason.versionMismatch ||
      updateReason == OtaUpdateReason.componentDrift) {
    return true;
  }

  final current = _canonicalizeOtaVersion(currentVersion);
  final latest = _canonicalizeOtaVersion(latestVersion);
  return current != null && latest != null && current != latest;
}

class OtaBundleEntry {
  final String title;
  final String? detail;

  const OtaBundleEntry({
    required this.title,
    this.detail,
  });

  factory OtaBundleEntry.fromJsonValue(Object? value) {
    if (value is Map) {
      final map = value.map(
        (key, entryValue) => MapEntry(key.toString(), entryValue),
      );
      final titleKey = _pickTitleKey(map);
      final title = titleKey != null
          ? _normalizeText(map[titleKey]) ?? titleKey
          : _firstNonEmptyText(map.values) ?? 'Unknown';
      final details = <String>[];

      for (final entry in map.entries) {
        if (entry.key == titleKey) continue;
        final text = _describeValue(entry.value);
        if (text == null) continue;
        details.add('${entry.key}: $text');
      }

      return OtaBundleEntry(
        title: title,
        detail: details.isEmpty ? null : details.join('  ·  '),
      );
    }

    if (value is Iterable) {
      final parts = value.map(_describeValue).whereType<String>().toList();
      if (parts.isNotEmpty) {
        return OtaBundleEntry(title: parts.join(', '));
      }
    }

    return OtaBundleEntry(
      title: _normalizeText(value) ?? 'Unknown',
    );
  }

  static List<OtaBundleEntry> listFromJson(Object? value) {
    if (value == null) return const [];
    if (value is List) {
      return List<OtaBundleEntry>.unmodifiable(
        value.map(OtaBundleEntry.fromJsonValue),
      );
    }
    if (value is Iterable) {
      return List<OtaBundleEntry>.unmodifiable(
        value.map(OtaBundleEntry.fromJsonValue),
      );
    }
    return List<OtaBundleEntry>.unmodifiable([
      OtaBundleEntry.fromJsonValue(value),
    ]);
  }

  static String? _pickTitleKey(Map<String, dynamic> map) {
    for (final key in const [
      'target',
      'name',
      'component',
      'id',
      'asset',
      'path',
      'url',
    ]) {
      if (_normalizeText(map[key]) != null) {
        return key;
      }
    }
    return null;
  }

  static String? _firstNonEmptyText(Iterable<Object?> values) {
    for (final value in values) {
      final text = _normalizeText(value);
      if (text != null) return text;
    }
    return null;
  }

  static String? _describeValue(Object? value) {
    if (value is Map) {
      final parts = value.entries
          .map((entry) {
            final text = _normalizeText(entry.value);
            return text == null ? null : '${entry.key}: $text';
          })
          .whereType<String>()
          .toList();
      return parts.isEmpty ? null : '{${parts.join(', ')}}';
    }

    if (value is Iterable) {
      final parts = value.map(_normalizeText).whereType<String>().toList();
      return parts.isEmpty ? null : '[${parts.join(', ')}]';
    }

    return _normalizeText(value);
  }

  static String? _normalizeText(Object? value) {
    final text = value?.toString().trim();
    if (text == null || text.isEmpty || text == 'null') {
      return null;
    }
    return text;
  }
}

Map<String, String>? _bearerAuthHeaders(String? authToken) {
  final token = authToken?.trim();
  if (token == null || token.isEmpty) return null;
  return {'Authorization': 'Bearer $token'};
}

/// Firmware release metadata from an update check.
class FirmwareRelease {
  final String version;
  final String url;
  final int? size;
  final String? changelog;
  final OtaUpdateReason updateReason;
  final List<OtaBundleEntry> installTargets;
  final List<OtaBundleEntry> imageAssets;

  FirmwareRelease({
    required this.version,
    this.url = '',
    this.size,
    this.changelog,
    this.updateReason = OtaUpdateReason.unknown,
    List<OtaBundleEntry> installTargets = const [],
    List<OtaBundleEntry> imageAssets = const [],
  })  : installTargets = List<OtaBundleEntry>.unmodifiable(installTargets),
        imageAssets = List<OtaBundleEntry>.unmodifiable(imageAssets);

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
  final List<String> payloads;
  final bool? supportsRootfsImage;

  const OtaCapabilities({
    required this.strategy,
    required this.scope,
    required this.canCheck,
    required this.canUpdate,
    required this.canUpload,
    required this.requiresRestart,
    required this.rollback,
    required this.payloads,
    required this.supportsRootfsImage,
  });

  factory OtaCapabilities.fromJson(Map<String, dynamic> json) {
    final payloads = (json['payloads'] as Iterable?)
            ?.map((value) => value?.toString().trim())
            .whereType<String>()
            .where((value) => value.isNotEmpty)
            .toList(growable: false) ??
        const <String>[];

    return OtaCapabilities(
      strategy: json['strategy']?.toString() ?? 'unknown',
      scope: json['scope']?.toString() ?? 'unknown',
      canCheck: json['can_check'] == true,
      canUpdate: json['can_update'] == true,
      canUpload: json['can_upload'] == true,
      requiresRestart: json['requires_restart'] == true,
      rollback: json['rollback']?.toString() ?? 'unknown',
      payloads: payloads,
      supportsRootfsImage: _tryParseBool(
        json['supports_rootfs_image'] ??
            json['supports_rootfs_image_ota'] ??
            json['rootfs_image_ota'] ??
            json['rootfs_image'],
      ),
    );
  }

  bool get supportsRootfsImageOta =>
      supportsRootfsImage ??
      payloads.contains('rootfs_image') ||
          scope == 'rootfs_image' ||
          scope == 'rootfs_slot';

  bool get supportsSelfPullUpdate =>
      strategy == 'self_pull' &&
      (_isSupportedScope || _hasSupportedPayloads) &&
      canCheck &&
      canUpdate &&
      !canUpload;

  bool get _isSupportedScope =>
      scope == 'binary' ||
      scope == 'bundle' ||
      scope == 'rootfs_image' ||
      scope == 'component_bundle' ||
      scope == 'rootfs_slot';

  bool get _hasSupportedPayloads =>
      payloads.contains('archive_bundle') || payloads.contains('rootfs_image');
}

class _OtaStatusPayload {
  final String state;
  final String currentVersion;
  final String latestVersion;
  final String? targetVersion;
  final bool updateAvailable;
  final bool hasCheckResult;
  final String? message;
  final String? lastError;
  final OtaUpdateReason? updateReason;
  final List<OtaBundleEntry>? installTargets;
  final List<OtaBundleEntry>? imageAssets;
  final List<OtaBundleEntry>? installedTargets;
  final bool? checksumVerified;

  const _OtaStatusPayload({
    required this.state,
    required this.currentVersion,
    required this.latestVersion,
    required this.targetVersion,
    required this.updateAvailable,
    required this.hasCheckResult,
    required this.message,
    required this.lastError,
    required this.updateReason,
    required this.installTargets,
    required this.imageAssets,
    required this.installedTargets,
    required this.checksumVerified,
  });

  factory _OtaStatusPayload.fromJson(Map<String, dynamic> json) {
    final currentVersion = json['current_version']?.toString() ?? '0.0.0';
    final latestVersion = json['latest_version']?.toString();
    final updateReason = json.containsKey('update_reason')
        ? _tryParseOtaUpdateReason(json['update_reason'])
        : null;
    final hasCheckResult = json.containsKey('checked_at_epoch_ms') ||
        json.containsKey('latest_version') ||
        json.containsKey('update_available') ||
        json.containsKey('update_reason');

    return _OtaStatusPayload(
      state: json['state']?.toString() ?? 'idle',
      currentVersion: currentVersion,
      latestVersion: latestVersion ?? '0.0.0',
      targetVersion: json['target_version']?.toString(),
      updateAvailable: _inferUpdateAvailable(
        updateAvailableValue: json['update_available'],
        currentVersion: currentVersion,
        latestVersion: latestVersion,
        updateReason: updateReason,
      ),
      hasCheckResult: hasCheckResult,
      message: json['message']?.toString(),
      lastError: json['last_error']?.toString(),
      updateReason: updateReason,
      installTargets: json.containsKey('install_targets')
          ? OtaBundleEntry.listFromJson(json['install_targets'])
          : null,
      imageAssets: json.containsKey('image_assets')
          ? OtaBundleEntry.listFromJson(json['image_assets'])
          : null,
      installedTargets: json.containsKey('installed_targets')
          ? OtaBundleEntry.listFromJson(json['installed_targets'])
          : null,
      checksumVerified: json.containsKey('checksum_verified')
          ? _tryParseBool(json['checksum_verified'])
          : null,
    );
  }
}

/// ChangeNotifier wrapper for OTA state used by the settings UI.
class OtaService extends ChangeNotifier {
  OtaService({
    Duration startUpdateReceiveTimeout = const Duration(minutes: 2),
    Duration startUpdateRecoveryWindow = const Duration(seconds: 20),
    Duration selfPullPollInterval = const Duration(seconds: 2),
  })  : _startUpdateReceiveTimeout = startUpdateReceiveTimeout,
        _startUpdateRecoveryWindow = startUpdateRecoveryWindow,
        _selfPullPollInterval = selfPullPollInterval;

  final sdk.RhythmOtaApi _legacyApi = sdk.RhythmOtaApi();
  final Duration _startUpdateReceiveTimeout;
  final Duration _startUpdateRecoveryWindow;
  final Duration _selfPullPollInterval;

  Dio? _dio;
  String? _host;
  String? _authToken;
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
  OtaUpdateReason _updateReason = OtaUpdateReason.unknown;
  List<OtaBundleEntry> _installTargets = const [];
  List<OtaBundleEntry> _imageAssets = const [];
  List<OtaBundleEntry> _installedTargets = const [];
  bool? _checksumVerified;
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
  OtaUpdateReason get updateReason => _updateReason;
  List<OtaBundleEntry> get installTargets =>
      List<OtaBundleEntry>.unmodifiable(_installTargets);
  List<OtaBundleEntry> get imageAssets =>
      List<OtaBundleEntry>.unmodifiable(_imageAssets);
  List<OtaBundleEntry> get installedTargets =>
      List<OtaBundleEntry>.unmodifiable(_installedTargets);
  bool? get checksumVerified => _checksumVerified;
  bool get isLoadingSupport => _isLoadingSupport;
  bool get showUpdateUi =>
      _strategy == _OtaStrategy.legacyUpload ||
      _strategy == _OtaStrategy.selfPull;
  bool get isSelfPull => _strategy == _OtaStrategy.selfPull;
  bool get isLegacyUpload => _strategy == _OtaStrategy.legacyUpload;
  bool get isProgressIndeterminate => _progress == null;
  bool get isBundleRepair => _updateReason == OtaUpdateReason.componentDrift;

  Future<void> initialize({
    required String host,
    int port = 80,
    String? fallbackCurrentVersion,
    String? fallbackPlatformType,
    String? fallbackPlatformContext,
    bool resetCheckStateOnInitialize = false,
    String? authToken,
  }) async {
    _configureClient(host, port, authToken);

    final fallbackVersion = _normalizeVersion(fallbackCurrentVersion);
    if (fallbackVersion != null) {
      _currentVersion = fallbackVersion;
    }

    _isLoadingSupport = true;
    _errorMessage = null;
    _notifyListeners();

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
      final isBridge = _looksLikeLegacyBridge(
        platformType: platformType,
        platformContext: platformContext,
      );
      _strategy = capabilities.supportsSelfPullUpdate && !isBridge
          ? _OtaStrategy.selfPull
          : (isBridge ? _OtaStrategy.legacyUpload : _OtaStrategy.unsupported);
    } catch (_) {
      _capabilities = null;
      _strategy = _looksLikeLegacyBridge(
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
      _updateReason = OtaUpdateReason.unknown;
      _installTargets = const [];
      _imageAssets = const [];
      _installedTargets = const [];
      _checksumVerified = null;
    }

    if (resetCheckStateOnInitialize) {
      _resetRestoredCheckState();
    }

    _isLoadingSupport = false;
    _notifyListeners();
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
    _updateReason = OtaUpdateReason.unknown;
    _installTargets = const [];
    _imageAssets = const [];
    _installedTargets = const [];
    _checksumVerified = null;
    _notifyListeners();

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

    _notifyListeners();
  }

  /// Start the selected update flow.
  Future<void> startUpdate(
    String deviceIp, {
    int port = 80,
    String? authToken,
  }) async {
    _configureClient(deviceIp, port, authToken ?? _authToken);
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
    _notifyListeners();

    _updateSub?.cancel();
    _updateSub = _legacyApi
        .startUpdate(release, deviceHost: deviceIp, port: port)
        .listen(
      _onLegacyProgress,
      onError: (Object error) {
        _state = OtaState.error;
        _errorMessage = error.toString();
        _progress = null;
        _notifyListeners();
      },
    );
  }

  Future<void> _checkSelfPullUpdate() async {
    _state = OtaState.checking;
    _errorMessage = null;
    _statusMessage = null;
    _progress = null;
    _updateReason = OtaUpdateReason.unknown;
    _installTargets = const [];
    _imageAssets = const [];
    _installedTargets = const [];
    _checksumVerified = null;
    _notifyListeners();

    try {
      final response = await _getJson('api/ota/check');
      final currentVersion =
          _normalizeVersion(response['current_version']?.toString()) ??
              _currentVersion;
      final latestVersion =
          _normalizeVersion(response['latest_version']?.toString()) ??
              currentVersion;
      final updateReason = response.containsKey('update_reason')
          ? _tryParseOtaUpdateReason(response['update_reason'])
          : null;
      final updateAvailable = _inferUpdateAvailable(
        updateAvailableValue: response['update_available'],
        currentVersion: currentVersion,
        latestVersion: latestVersion,
        updateReason: updateReason,
      );

      _currentVersion = currentVersion;
      _latestVersion = latestVersion;
      _targetVersion = updateAvailable ? latestVersion : null;
      _applySelfPullMetadataFromJson(response);

      if (updateAvailable) {
        _availableRelease = _buildSelfPullRelease(latestVersion);
        _state = OtaState.available;
      } else {
        _availableRelease = null;
        _state = OtaState.upToDate;
      }
    } catch (e) {
      _state = OtaState.error;
      _errorMessage = _formatError('Failed to check for updates', e);
    }

    _notifyListeners();
  }

  Future<void> _startSelfPullUpdate() async {
    if (_strategy != _OtaStrategy.selfPull) return;

    _errorMessage = null;
    _statusMessage = 'Starting update...';
    _progress = null;
    _installedTargets = const [];
    _checksumVerified = null;
    _state = OtaState.uploading;
    _notifyListeners();

    try {
      final response = await _dio!.post(
        'api/ota/update',
        options: Options(receiveTimeout: _startUpdateReceiveTimeout),
      );
      if ((response.statusCode ?? 500) != 200) {
        throw StateError('Unexpected response: ${response.statusCode}');
      }
      final data = response.data;
      if (data is Map) {
        _applySelfPullMetadataFromJson(
          data.map((key, value) => MapEntry(key.toString(), value)),
        );
      }
    } catch (e) {
      final recovered = await _recoverSelfPullStartAfterError(e);
      if (recovered) {
        if (_isTerminalState(_state)) {
          return;
        }
        unawaited(_pollSelfPullStatusUntilComplete());
        return;
      }

      _state = OtaState.error;
      _errorMessage = _formatError('Failed to start update', e);
      _notifyListeners();
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

    while (!_disposed &&
        token == _selfPullPollToken &&
        DateTime.now().isBefore(deadline)) {
      try {
        final status = await _fetchSelfPullStatus();
        _applySelfPullStatus(status, allowIdleReset: false);

        if (status.state == 'error') {
          return;
        }

        if (_statusShowsCompletedUpdate(status)) {
          _markComplete(_completedVersionForStatus(status));
          return;
        }
      } catch (e) {
        if (_shouldTreatStatusPollErrorAsTransitional(e)) {
          _state = OtaState.rebooting;
          _statusMessage = 'Device restarting';
          _progress = null;
          _notifyListeners();
        } else {
          _state = OtaState.error;
          _errorMessage = _formatError('Failed to poll update status', e);
          _statusMessage = null;
          _notifyListeners();
          return;
        }
      }

      await Future.delayed(_selfPullPollInterval);
    }

    if (!_disposed &&
        token == _selfPullPollToken &&
        !_isTerminalState(_state)) {
      _state = OtaState.error;
      _errorMessage = 'Device did not come back online after update';
      _statusMessage = null;
      _notifyListeners();
    }
  }

  Future<_OtaStatusPayload> _fetchSelfPullStatus() async {
    final json = await _getJson('api/ota/status');
    return _OtaStatusPayload.fromJson(json);
  }

  Future<bool> _recoverSelfPullStartAfterError(Object error) async {
    if (!_shouldProbeStartProgressAfterRequestError(error)) {
      return false;
    }

    final deadline = DateTime.now().add(_startUpdateRecoveryWindow);
    _statusMessage = 'Waiting for device to begin update...';
    _notifyListeners();

    while (!_disposed && DateTime.now().isBefore(deadline)) {
      try {
        final status = await _fetchSelfPullStatus();

        if (_statusShowsAcceptedUpdateRequest(status)) {
          _applySelfPullStatus(status, allowIdleReset: false);
          return true;
        }

        if (_statusShowsCompletedUpdate(status)) {
          _markComplete(_completedVersionForStatus(status));
          return true;
        }
      } catch (statusError) {
        if (_shouldTreatStatusPollErrorAsTransitional(statusError)) {
          _state = OtaState.rebooting;
          _statusMessage = 'Device restarting';
          _progress = null;
          _notifyListeners();
          return true;
        }
      }

      await Future.delayed(_selfPullPollInterval);
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
    if (status.updateReason != null) {
      _updateReason = status.updateReason!;
    }
    if (status.installTargets != null) {
      _installTargets = status.installTargets!;
    }
    if (status.imageAssets != null) {
      _imageAssets = status.imageAssets!;
    }
    if (status.installedTargets != null) {
      _installedTargets = status.installedTargets!;
    }
    if (status.checksumVerified != null) {
      _checksumVerified = status.checksumVerified;
    }
    _availableRelease = status.updateAvailable
        ? _buildSelfPullRelease(_latestVersion ?? _currentVersion)
        : null;
    _statusMessage = status.message;
    _progress = null;

    if (status.state == 'checking') {
      _state = OtaState.checking;
    } else if (status.state == 'ready') {
      if (status.updateAvailable) {
        _state = OtaState.available;
      } else {
        _state = OtaState.upToDate;
      }
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
      if (allowIdleReset) {
        if (status.updateAvailable) {
          _state = OtaState.available;
        } else if (!status.hasCheckResult) {
          _availableRelease = null;
          _state = OtaState.idle;
        } else {
          _availableRelease = null;
          _state = OtaState.upToDate;
        }
      }
    }

    _notifyListeners();
  }

  void _onLegacyProgress(sdk.RhythmOtaProgress event) {
    _state = _mapLegacyState(event.state);
    _progress = event.progressPercent;
    _errorMessage = event.errorMessage;
    _statusMessage = event.errorMessage;
    if (_state == OtaState.complete && _latestVersion != null) {
      _currentVersion = _latestVersion!;
    }
    _notifyListeners();
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
    _availableRelease = null;
    _statusMessage = _updateReason == OtaUpdateReason.componentDrift
        ? 'Bundle repaired on ${_formatVersionForDisplay(_currentVersion)}'
        : 'Updated to ${_formatVersionForDisplay(_currentVersion)}';
    _errorMessage = null;
    _progress = 100;
    _state = OtaState.complete;
    _notifyListeners();
  }

  FirmwareRelease _buildSelfPullRelease(String version) {
    return FirmwareRelease(
      version: version,
      updateReason: _updateReason,
      installTargets: _installTargets,
      imageAssets: _imageAssets,
    );
  }

  void _applySelfPullMetadataFromJson(Map<String, dynamic> json) {
    final updateReason = _tryParseOtaUpdateReason(json['update_reason']);
    if (updateReason != null) {
      _updateReason = updateReason;
    }
    if (json.containsKey('install_targets')) {
      _installTargets = OtaBundleEntry.listFromJson(json['install_targets']);
    }
    if (json.containsKey('image_assets')) {
      _imageAssets = OtaBundleEntry.listFromJson(json['image_assets']);
    }
    if (json.containsKey('installed_targets')) {
      _installedTargets =
          OtaBundleEntry.listFromJson(json['installed_targets']);
    }
    if (json.containsKey('checksum_verified')) {
      _checksumVerified = _tryParseBool(json['checksum_verified']);
    }
  }

  void _configureClient(String host, int port, String? authToken) {
    if (_host == host &&
        _port == port &&
        _authToken == authToken &&
        _dio != null) {
      return;
    }

    _host = host;
    _port = port;
    _authToken = authToken;
    _dio?.close();
    _dio = Dio(BaseOptions(
      baseUrl: port == 80 ? 'http://$host/' : 'http://$host:$port/',
      connectTimeout: const Duration(seconds: 5),
      receiveTimeout: const Duration(seconds: 10),
      headers: _bearerAuthHeaders(authToken),
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

  bool _looksLikeLegacyBridge({
    String? platformType,
    String? platformContext,
  }) {
    return platformType == 'bridge' ||
        platformContext == 'bridge' ||
        platformType == 'embedded' ||
        platformContext == 'embedded';
  }

  bool _statusShowsCompletedUpdate(_OtaStatusPayload status) {
    if (status.state != 'idle' && status.state != 'ready') return false;
    if (status.updateAvailable) return false;

    final lastError = status.lastError?.trim();
    if (lastError != null && lastError.isNotEmpty) return false;

    if (_updateReason == OtaUpdateReason.componentDrift) {
      return true;
    }

    final current = _canonicalizeVersion(status.currentVersion);
    final expected = _canonicalizeVersion(status.targetVersion) ??
        _canonicalizeVersion(_targetVersion) ??
        _canonicalizeVersion(status.latestVersion) ??
        _canonicalizeVersion(_latestVersion);
    return current != null && expected != null && current == expected;
  }

  String _completedVersionForStatus(_OtaStatusPayload status) {
    final current = _normalizeVersion(status.currentVersion);
    if (_statusShowsCompletedUpdate(status) && current != null) {
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

  void _resetRestoredCheckState() {
    if (_isUpdateInProgress(_state) || _state == OtaState.checking) {
      return;
    }

    if (_state == OtaState.available ||
        _state == OtaState.upToDate ||
        _state == OtaState.complete) {
      reset();
    }
  }

  bool _statusShowsAcceptedUpdateRequest(_OtaStatusPayload status) =>
      status.state == 'checking' ||
      status.state == 'updating' ||
      status.state == 'restarting' ||
      status.state == 'error';

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

  bool _shouldProbeStartProgressAfterRequestError(Object error) {
    if (error is DioException) {
      if (error.type == DioExceptionType.connectionTimeout ||
          error.type == DioExceptionType.receiveTimeout) {
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
        message.contains('connection reset') ||
        message.contains('connection closed') ||
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

  String _formatVersionForDisplay(String version) {
    return version.startsWith('v') || version.startsWith('V')
        ? version
        : 'v$version';
  }

  void _notifyListeners() {
    if (_disposed) return;
    notifyListeners();
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
    _updateReason = OtaUpdateReason.unknown;
    _installTargets = const [];
    _imageAssets = const [];
    _installedTargets = const [];
    _checksumVerified = null;
    if (_strategy == _OtaStrategy.selfPull) {
      _latestVersion = _currentVersion;
    }
    _notifyListeners();
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
