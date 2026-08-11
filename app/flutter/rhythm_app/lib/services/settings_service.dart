import 'dart:async';
import 'dart:convert';
import 'package:flutter/foundation.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_core/runner/runner_state_json.dart' as runner_json;
import '../data/local_data_source.dart';
import 'server_identity.dart';

/// Appliance scope resolved from the active connection's hello identity.
/// A persisted durable identity may fill the pre-hello gap, but a conflict
/// between persisted and connected durable identities is never guessed away.
/// An unidentified Hub never produces a persistence scope.
String? localBlePairingConnectedServerScope(
  Hub hub,
  String? connectedServerInstanceId,
) {
  final persisted = normalizeServerIdentity(hub.serverInstanceId);
  final connected = normalizeServerIdentity(connectedServerInstanceId);
  final persistedDurable =
      serverIdentityKind(persisted) == ServerIdentityKind.durable
          ? persisted
          : null;
  final connectedDurable =
      serverIdentityKind(connected) == ServerIdentityKind.durable
          ? connected
          : null;
  if (persistedDurable != null &&
      connectedDurable != null &&
      persistedDurable != connectedDurable) {
    return null;
  }
  final identity = connectedDurable ?? persistedDurable;
  return identity == null ? null : '${hub.homeId}:$identity';
}

/// Stable in-memory route anchor used only while the first durable hello is
/// still in flight. Local-BLE recovery pointers must never use this scope.
String localBlePairingFallbackServerScope(Hub hub) => '${hub.homeId}:${hub.id}';

const Set<String> _stableLocalBleAssociationFailureStages = {
  'target_not_observed',
  'candidate_open',
  'candidate_connect',
  'candidate_service_discovery',
  'candidate_service_mismatch',
  'candidate_cleanup',
  'transport',
};

const Set<String> _persistedLocalBleFailureStages = {
  ..._stableLocalBleAssociationFailureStages,
  'pairing',
  'terminal_event',
  'terminal_event_missing_device',
  'terminal_status',
  'terminal_status_missing_device',
  'terminal_status_not_found',
  'invalid_response',
};

/// Accepts only the appliance's stable, privacy-safe local-BLE stage catalog.
///
/// Newer servers may add categories. Older apps deliberately collapse those
/// values to their existing bounded fallback rather than creating unbounded
/// analytics values or persisting an unreviewed diagnostic string.
String localBleFailureStageOrFallback(
  String? serverFailureStage, {
  required String fallback,
}) {
  final stage = serverFailureStage?.trim();
  return _stableLocalBleAssociationFailureStages.contains(stage)
      ? stage!
      : fallback;
}

/// Sanitized terminal outcome cached before the appliance result is
/// acknowledged. This closes both crash windows around server acknowledgement:
/// recovery can render this result without depending on another GET, then
/// retry DELETE and the local pointer clear independently.
class PendingLocalBleTerminalResult {
  const PendingLocalBleTerminalResult({
    required this.status,
    this.deviceId,
    this.deviceName,
    this.deviceType,
    this.manufacturer,
    this.model,
    this.warnings = const [],
    this.error,
    this.failureStage,
  });

  static const String complete = 'complete';
  static const String failed = 'failed';
  static const String notFound = 'not_found';

  final String status;
  final String? deviceId;
  final String? deviceName;
  final String? deviceType;
  final String? manufacturer;
  final String? model;
  final List<String> warnings;
  final String? error;
  final String? failureStage;

  bool hasSameValue(PendingLocalBleTerminalResult other) =>
      status == other.status &&
      deviceId == other.deviceId &&
      deviceName == other.deviceName &&
      deviceType == other.deviceType &&
      manufacturer == other.manufacturer &&
      model == other.model &&
      listEquals(warnings, other.warnings) &&
      error == other.error &&
      failureStage == other.failureStage;

  Map<String, Object> toJson() => {
        'status': status,
        if (deviceId != null) 'device_id': deviceId!,
        if (deviceName != null) 'device_name': deviceName!,
        if (deviceType != null) 'device_type': deviceType!,
        if (manufacturer != null) 'manufacturer': manufacturer!,
        if (model != null) 'model': model!,
        if (warnings.isNotEmpty) 'warnings': warnings,
        if (error != null) 'error': error!,
        if (failureStage != null) 'failure_stage': failureStage!,
      };

  static PendingLocalBleTerminalResult? fromJson(Object? value) {
    if (value is! Map) return null;
    final status = value['status'];
    if (status != complete && status != failed && status != notFound) {
      return null;
    }
    final deviceId = _boundedOptionalTerminalString(
      value['device_id'],
      maxLength: 256,
    );
    final deviceName = _boundedOptionalTerminalString(
      value['device_name'],
      maxLength: 256,
    );
    final deviceType = _boundedOptionalTerminalString(
      value['device_type'],
      maxLength: 64,
    );
    final manufacturer = _boundedOptionalTerminalString(
      value['manufacturer'],
      maxLength: 128,
      allowEmpty: true,
    );
    final model = _boundedOptionalTerminalString(
      value['model'],
      maxLength: 128,
      allowEmpty: true,
    );
    final error = _boundedOptionalTerminalString(
      value['error'],
      maxLength: 512,
      allowEmpty: true,
    );
    final failureStage = _boundedOptionalTerminalString(
      value['failure_stage'],
      maxLength: 64,
    );
    if ((value['device_id'] != null && deviceId == null) ||
        (value['device_name'] != null && deviceName == null) ||
        (value['device_type'] != null && deviceType == null) ||
        (value['manufacturer'] != null && manufacturer == null) ||
        (value['model'] != null && model == null) ||
        (value['error'] != null && error == null) ||
        (value['failure_stage'] != null &&
            (failureStage == null ||
                !_persistedLocalBleFailureStages.contains(failureStage)))) {
      return null;
    }
    final rawWarnings = value['warnings'];
    final warnings = <String>[];
    if (rawWarnings != null) {
      if (rawWarnings is! List || rawWarnings.length > 32) return null;
      for (final warning in rawWarnings) {
        if (warning is! String ||
            warning.trim().isEmpty ||
            warning.length > 512) {
          return null;
        }
        warnings.add(warning);
      }
    }

    final hasAnyDeviceField = deviceId != null ||
        deviceName != null ||
        deviceType != null ||
        manufacturer != null ||
        model != null;
    if (status == complete) {
      if (deviceId == null ||
          deviceName == null ||
          deviceType == null ||
          error != null ||
          failureStage != null) {
        return null;
      }
    } else if (hasAnyDeviceField || warnings.isNotEmpty) {
      return null;
    }

    return PendingLocalBleTerminalResult(
      status: status,
      deviceId: deviceId,
      deviceName: deviceName,
      deviceType: deviceType,
      manufacturer: manufacturer,
      model: model,
      warnings: List.unmodifiable(warnings),
      error: error,
      failureStage: failureStage,
    );
  }
}

String? _boundedOptionalTerminalString(
  Object? value, {
  required int maxLength,
  bool allowEmpty = false,
}) {
  if (value == null) return null;
  if (value is! String ||
      value.length > maxLength ||
      (!allowEmpty && value.trim().isEmpty)) {
    return null;
  }
  return value;
}

/// Privacy-bounded app-side pointer to an appliance pairing operation.
/// Setup payloads must never be added here. A bounded, sanitized terminal
/// result may be cached only after the appliance finishes the operation.
class PendingLocalBlePairing {
  const PendingLocalBlePairing({
    required this.sessionId,
    required this.journeyId,
    required this.attemptNumber,
    required this.profileId,
    required this.serverScope,
    required this.startedAtEpochMs,
    this.terminalResult,
  });

  final String sessionId;
  final String journeyId;
  final int attemptNumber;
  final String profileId;

  /// Stable `home_id:server_instance_id` scope. It is a logical appliance
  /// identity, never a network address or physical BLE identity.
  final String serverScope;
  final int startedAtEpochMs;
  final PendingLocalBleTerminalResult? terminalResult;

  Map<String, Object> toJson() => {
        'session_id': sessionId,
        'journey_id': journeyId,
        'attempt_number': attemptNumber,
        'profile_id': profileId,
        'server_scope': serverScope,
        'started_at_epoch_ms': startedAtEpochMs,
        if (terminalResult != null) 'terminal_result': terminalResult!.toJson(),
      };

  PendingLocalBlePairing withTerminalResult(
    PendingLocalBleTerminalResult result,
  ) =>
      PendingLocalBlePairing(
        sessionId: sessionId,
        journeyId: journeyId,
        attemptNumber: attemptNumber,
        profileId: profileId,
        serverScope: serverScope,
        startedAtEpochMs: startedAtEpochMs,
        terminalResult: result,
      );

  static PendingLocalBlePairing? fromJson(Object? value) {
    if (value is! Map) return null;
    final sessionId = value['session_id'];
    final journeyId = value['journey_id'];
    final attemptNumber = value['attempt_number'];
    final profileId = value['profile_id'];
    final serverScope = value['server_scope'];
    final startedAt = value['started_at_epoch_ms'];
    final rawTerminalResult = value['terminal_result'];
    final terminalResult = rawTerminalResult == null
        ? null
        : PendingLocalBleTerminalResult.fromJson(rawTerminalResult);
    if (sessionId is! String ||
        sessionId.isEmpty ||
        sessionId.length > 96 ||
        !RegExp(r'^[A-Za-z0-9_.:-]+$').hasMatch(sessionId) ||
        journeyId is! String ||
        journeyId.isEmpty ||
        journeyId.length > 96 ||
        !RegExp(r'^[A-Za-z0-9_.:-]+$').hasMatch(journeyId) ||
        attemptNumber is! int ||
        attemptNumber < 1 ||
        attemptNumber > 100 ||
        profileId is! String ||
        profileId.isEmpty ||
        profileId.length > 128 ||
        serverScope is! String ||
        serverScope.isEmpty ||
        serverScope.length > 256 ||
        startedAt is! int ||
        startedAt < 0 ||
        (rawTerminalResult != null && terminalResult == null)) {
      return null;
    }
    return PendingLocalBlePairing(
      sessionId: sessionId,
      journeyId: journeyId,
      attemptNumber: attemptNumber,
      profileId: profileId,
      serverScope: serverScope,
      startedAtEpochMs: startedAt,
      terminalResult: terminalResult,
    );
  }
}

/// Process-local ownership fence between the active pairing route and the
/// app-shell recovery coordinator. Durable storage remains authoritative
/// across process death; this fence only prevents two live UI surfaces from
/// consuming the same terminal result.
class LocalBlePairingRouteOwnership {
  LocalBlePairingRouteOwnership._();

  static final ValueNotifier<Set<String>> ownedKeys =
      ValueNotifier<Set<String>>(const {});
  static final Map<String, int> _claimCounts = {};

  static String key(String serverScope, String sessionId) =>
      '$serverScope\u0000$sessionId';

  static void claim(String serverScope, String sessionId) {
    final pairingKey = key(serverScope, sessionId);
    final count = _claimCounts[pairingKey] ?? 0;
    _claimCounts[pairingKey] = count + 1;
    if (count > 0) return;
    ownedKeys.value = {...ownedKeys.value, pairingKey};
  }

  static void release(String serverScope, String sessionId) {
    final pairingKey = key(serverScope, sessionId);
    final count = _claimCounts[pairingKey] ?? 0;
    if (count > 1) {
      _claimCounts[pairingKey] = count - 1;
      return;
    }
    _claimCounts.remove(pairingKey);
    if (!ownedKeys.value.contains(pairingKey)) return;
    ownedKeys.value = {
      for (final value in ownedKeys.value)
        if (value != pairingKey) value,
    };
  }
}

/// Singleton service for device-specific settings.
///
/// This service:
/// - Initializes before backend (for onboardingComplete check)
/// - Performs one-time migration from SharedPreferences to Hive
/// - Provides getters/setters for all device-specific settings
///
/// Location, sleep schedule, and timezone are stored in the Home model
/// (via HomeProvider) and sync to cloud. This service only handles
/// device-local settings.
class SettingsService {
  static const String roomPageLayoutScopePrefix = 'room_page_layout::';
  static const String _roomPageLayoutCloudDirtyPrefix =
      'room_page_layout_cloud_dirty::';
  static const String selectedHomeIdKey = 'selected_home_id';
  static const String _handledPasswordRecoveryLinksKey =
      'handled_password_recovery_links_v1';
  static const int _maxHandledPasswordRecoveryLinks = 20;
  static const String _pendingLocalBlePairingKey =
      'pending_local_ble_pairings_v4';
  static const int _maxPendingLocalBlePairingScopes = 8;
  static const int _maxPendingLocalBlePairingEncodedChars = 256 * 1024;

  static SettingsService? _instance;
  static SettingsService get instance => _instance ??= SettingsService._();

  SettingsService._();

  LocalDataSource? _localDataSource;
  AppSettings _settings = AppSettings.defaults();
  bool _initialized = false;
  Future<void> _pendingLocalBleMutationTail = Future.value();

  /// Whether the service has been initialized.
  bool get isInitialized => _initialized;

  /// Get the current settings.
  AppSettings get settings => _settings;

  // ============================================================
  // Initialization
  // ============================================================

  /// Initialize the settings service.
  ///
  /// This must be called before accessing settings. It will:
  /// 1. Initialize LocalDataSource if needed
  /// 2. Perform one-time migration from SharedPreferences
  /// 3. Load settings from Hive
  Future<void> initialize({LocalDataSource? localDataSource}) async {
    if (_initialized) return;

    debugPrint('SettingsService: Initializing...');

    // Use provided data source or create new one
    _localDataSource = localDataSource ?? LocalDataSource();
    if (!_localDataSource!.isInitialized) {
      await _localDataSource!.initialize();
    }

    // Migrate from SharedPreferences if needed
    if (!_localDataSource!.isMigrationComplete()) {
      await _migrateFromSharedPreferences();
    }

    // Load settings
    _settings = _localDataSource!.getSettings();
    _initialized = true;

    debugPrint(
        'SettingsService: Initialized (onboardingComplete=${_settings.onboardingComplete})');
  }

  /// Migrate settings from SharedPreferences to Hive.
  ///
  /// This is a one-time operation that preserves all existing settings.
  Future<void> _migrateFromSharedPreferences() async {
    debugPrint('SettingsService: Migrating from SharedPreferences...');

    try {
      final prefs = await SharedPreferences.getInstance();

      // Read all SP values
      final use24HourFormat = prefs.getBool('use24HourFormat') ?? false;
      final onboardingComplete = prefs.getBool('onboardingComplete') ?? false;
      final notificationsEnabled =
          prefs.getBool('notificationsEnabled') ?? false;
      final hueSseEnabled = prefs.getBool('hue_sse_enabled') ?? true;

      // Read JSON data
      final hueDeviceRegistryJson = prefs.getString('hue_sse_device_registry');
      final curveConfigJson = prefs.getString('rhythm_curve_config');
      final runnerStateJson = prefs.getString('rhythm_runner_state');

      // Create settings object
      _settings = AppSettings(
        use24HourFormat: use24HourFormat,
        onboardingComplete: onboardingComplete,
        notificationsEnabled: notificationsEnabled,
        hueSseEnabled: hueSseEnabled,
        hueDeviceRegistryJson: hueDeviceRegistryJson,
        curveConfigJson: curveConfigJson,
        runnerStateJson: runnerStateJson,
      );

      // Save to Hive
      await _localDataSource!.saveSettings(_settings);
      await _localDataSource!.markMigrationComplete();

      debugPrint('SettingsService: Migration complete');
    } catch (e) {
      debugPrint('SettingsService: Migration failed: $e');
      // Continue with defaults - don't block app startup
    }
  }

  // ============================================================
  // Boolean Settings
  // ============================================================

  /// Whether to use 24-hour time format.
  bool get use24HourFormat => _settings.use24HourFormat;

  /// Set 24-hour time format preference.
  Future<void> setUse24HourFormat(bool value) async {
    _settings = _settings.copyWith(use24HourFormat: value);
    await _save();
  }

  /// Toggle 24-hour time format.
  Future<void> toggleTimeFormat() async {
    await setUse24HourFormat(!_settings.use24HourFormat);
  }

  /// Whether onboarding has been completed.
  bool get onboardingComplete => _settings.onboardingComplete;

  /// Mark onboarding as complete.
  Future<void> setOnboardingComplete(bool value) async {
    if (_settings.onboardingComplete == value) return;
    _settings = _settings.copyWith(onboardingComplete: value);
    await _save();
  }

  /// Whether push notifications are enabled.
  bool get notificationsEnabled => _settings.notificationsEnabled;

  /// Set notifications preference.
  Future<void> setNotificationsEnabled(bool value) async {
    _settings = _settings.copyWith(notificationsEnabled: value);
    await _save();
  }

  Future<bool> hasHandledPasswordRecoveryLink(String fingerprint) async {
    if (fingerprint.isEmpty) return false;
    try {
      final prefs = await SharedPreferences.getInstance();
      final handled =
          prefs.getStringList(_handledPasswordRecoveryLinksKey) ?? const [];
      return handled.contains(fingerprint);
    } catch (_) {
      return false;
    }
  }

  Future<void> markPasswordRecoveryLinkHandled(String fingerprint) async {
    if (fingerprint.isEmpty) return;
    try {
      final prefs = await SharedPreferences.getInstance();
      final handled = List<String>.from(
        prefs.getStringList(_handledPasswordRecoveryLinksKey) ??
            const <String>[],
      )..remove(fingerprint);
      handled.add(fingerprint);
      final trimmed = handled.length > _maxHandledPasswordRecoveryLinks
          ? handled.sublist(handled.length - _maxHandledPasswordRecoveryLinks)
          : handled;
      await prefs.setStringList(_handledPasswordRecoveryLinksKey, trimmed);
    } catch (_) {}
  }

  /// Whether Hue SSE is enabled.
  bool get hueSseEnabled => _settings.hueSseEnabled;

  /// Set Hue SSE preference.
  Future<void> setHueSseEnabled(bool value) async {
    _settings = _settings.copyWith(hueSseEnabled: value);
    await _save();
  }

  // ============================================================
  // Electricity Rate
  // ============================================================

  /// Electricity rate in currency per kWh (e.g. 0.12 for $0.12/kWh).
  double? get electricityRate => _settings.electricityRate;

  /// Set electricity rate.
  Future<void> setElectricityRate(double? value) async {
    _settings = _settings.copyWith(electricityRate: value);
    await _save();
  }

  // ============================================================
  // JSON Data Storage
  // ============================================================

  /// Get Hue device registry JSON.
  String? get hueDeviceRegistryJson => _settings.hueDeviceRegistryJson;

  /// Get Hue device registry as Map.
  Map<String, dynamic>? getHueDeviceRegistry() {
    final json = _settings.hueDeviceRegistryJson;
    if (json == null) return null;
    try {
      return jsonDecode(json) as Map<String, dynamic>;
    } catch (e) {
      return null;
    }
  }

  /// Save Hue device registry.
  Future<void> saveHueDeviceRegistry(Map<String, dynamic> registry) async {
    _settings = _settings.copyWith(hueDeviceRegistryJson: jsonEncode(registry));
    await _save();
  }

  /// Clear Hue device registry.
  Future<void> clearHueDeviceRegistry() async {
    _settings = _settings.clearField(clearHueDeviceRegistryJson: true);
    await _save();
  }

  /// Get the Hue `grouped_light` map keyed by room ID.
  Map<String, String>? getHueGroupedLightMap() {
    final json = _settings.hueGroupedLightMapJson;
    if (json == null) return null;
    try {
      final decoded = jsonDecode(json) as Map<String, dynamic>;
      return decoded.map((k, v) => MapEntry(k, v as String));
    } catch (e) {
      return null;
    }
  }

  /// Save Hue grouped_light map.
  Future<void> saveHueGroupedLightMap(Map<String, String> map) async {
    _settings = _settings.copyWith(hueGroupedLightMapJson: jsonEncode(map));
    await _save();
  }

  /// Clear Hue grouped_light map.
  Future<void> clearHueGroupedLightMap() async {
    _settings = _settings.clearField(clearHueGroupedLightMapJson: true);
    await _save();
  }

  /// Get curve config JSON.
  String? get curveConfigJson => _settings.curveConfigJson;

  /// Get curve config as RawConfig.
  RawConfig? getCurveConfig() {
    final json = _settings.curveConfigJson;
    if (json == null) return null;
    try {
      return RawConfig.fromJson(jsonDecode(json) as Map<String, dynamic>);
    } catch (e) {
      return null;
    }
  }

  /// Save curve config.
  Future<void> saveCurveConfig(RawConfig config) async {
    _settings =
        _settings.copyWith(curveConfigJson: jsonEncode(config.toJson()));
    await _save();
  }

  /// Clear curve config.
  Future<void> clearCurveConfig() async {
    _settings = _settings.clearField(clearCurveConfigJson: true);
    await _save();
  }

  /// Get runner state JSON.
  String? get runnerStateJson => _settings.runnerStateJson;

  /// Get runner state.
  RunnerStateDto? getRunnerState() {
    final json = _settings.runnerStateJson;
    if (json == null) return null;
    try {
      return runner_json.runnerStateFromJson(json);
    } catch (e) {
      return null;
    }
  }

  /// Save runner state.
  Future<void> saveRunnerState(RunnerStateDto state) async {
    final json = runner_json.runnerStateToJson(state);
    _settings = _settings.copyWith(runnerStateJson: json);
    await _save();
  }

  /// Clear runner state.
  Future<void> clearRunnerState() async {
    _settings = _settings.clearField(clearRunnerStateJson: true);
    await _save();
  }

  /// Device-local Home selection restored on app startup.
  String? get selectedHomeId {
    if (_localDataSource?.isInitialized != true) return null;
    final value = _localDataSource?.getSettingsValue(selectedHomeIdKey);
    if (value is String && value.isNotEmpty) return value;
    return null;
  }

  /// Save the last active Home on this device.
  Future<void> setSelectedHomeId(String? homeId) async {
    if (_localDataSource?.isInitialized != true) return;
    if (homeId == null || homeId.isEmpty) {
      await _localDataSource!.deleteSettingsValue(selectedHomeIdKey);
      return;
    }
    await _localDataSource!.saveSettingsValue(selectedHomeIdKey, homeId);
  }

  @visibleForTesting
  static List<PendingLocalBlePairing> decodePendingLocalBlePairings(
    Object? raw,
  ) {
    if (raw is String && raw.length > _maxPendingLocalBlePairingEncodedChars) {
      throw const FormatException(
        'Pending local-BLE pairing storage is too large.',
      );
    }
    final decoded = raw is String ? jsonDecode(raw) : raw;
    if (decoded is! List) {
      throw const FormatException(
        'Pending local-BLE pairing storage must be a list.',
      );
    }
    if (decoded.length > _maxPendingLocalBlePairingScopes) {
      throw const FormatException(
        'Pending local-BLE pairing storage exceeds its scope limit.',
      );
    }
    final byScope = <String, PendingLocalBlePairing>{};
    for (final value in decoded) {
      final record = PendingLocalBlePairing.fromJson(value);
      if (record == null) {
        throw const FormatException(
          'Pending local-BLE pairing storage contains an invalid record.',
        );
      }
      if (byScope.containsKey(record.serverScope)) {
        throw const FormatException(
          'Pending local-BLE pairing storage contains duplicate scopes.',
        );
      }
      byScope[record.serverScope] = record;
    }
    final records = byScope.values.toList()
      ..sort((left, right) =>
          left.startedAtEpochMs.compareTo(right.startedAtEpochMs));
    return records;
  }

  @visibleForTesting
  static String encodePendingLocalBlePairings(
    Iterable<PendingLocalBlePairing> records,
  ) {
    final values = records.map((record) => record.toJson()).toList();
    if (values.length > _maxPendingLocalBlePairingScopes) {
      throw const FormatException(
        'Pending local-BLE pairing storage exceeds its scope limit.',
      );
    }
    final encoded = jsonEncode(values);
    if (encoded.length > _maxPendingLocalBlePairingEncodedChars) {
      throw const FormatException(
        'Pending local-BLE pairing storage is too large.',
      );
    }
    return encoded;
  }

  /// Applies same-scope compare-and-set and capacity rules without evicting
  /// another unresolved operation. A null result means the write conflicts.
  @visibleForTesting
  static List<PendingLocalBlePairing>? mergePendingLocalBlePairingForSave(
    List<PendingLocalBlePairing> existing,
    PendingLocalBlePairing pairing,
  ) {
    final matching = existing
        .where((record) => record.serverScope == pairing.serverScope)
        .toList(growable: false);
    if (matching.any((record) => record.sessionId != pairing.sessionId)) {
      return null;
    }
    final existingTerminals = matching
        .where((record) => record.terminalResult != null)
        .toList(growable: false);
    if (existingTerminals.isNotEmpty) {
      final replacementTerminal = pairing.terminalResult;
      if (replacementTerminal == null) return null;
      for (final record in existingTerminals) {
        if (!record.terminalResult!.hasSameValue(replacementTerminal) ||
            record.journeyId != pairing.journeyId ||
            record.attemptNumber != pairing.attemptNumber ||
            record.profileId != pairing.profileId ||
            record.startedAtEpochMs != pairing.startedAtEpochMs) {
          return null;
        }
      }
    }
    if (matching.isEmpty &&
        existing.length >= _maxPendingLocalBlePairingScopes) {
      return null;
    }
    final normalized = PendingLocalBlePairing(
      sessionId: pairing.sessionId,
      journeyId: pairing.journeyId,
      attemptNumber: pairing.attemptNumber,
      profileId: pairing.profileId,
      serverScope: pairing.serverScope,
      startedAtEpochMs: pairing.startedAtEpochMs,
      terminalResult: pairing.terminalResult,
    );
    return [...existing]
      ..removeWhere((record) => record.serverScope == pairing.serverScope)
      ..add(normalized)
      ..sort((left, right) =>
          left.startedAtEpochMs.compareTo(right.startedAtEpochMs));
  }

  /// Load every unresolved pointer, independently scoped by appliance.
  Future<List<PendingLocalBlePairing>> loadPendingLocalBlePairings() async {
    if (_localDataSource?.isInitialized != true) return const [];
    final raw = _localDataSource!.getSettingsValue(_pendingLocalBlePairingKey);
    if (raw == null) return const [];
    return decodePendingLocalBlePairings(raw);
  }

  Future<PendingLocalBlePairing?> loadPendingLocalBlePairing(
    String serverScope,
  ) async {
    final records = await loadPendingLocalBlePairings();
    for (final record in records) {
      if (record.serverScope == serverScope) return record;
    }
    return null;
  }

  /// Save the operation pointer or its bounded terminal recovery snapshot.
  Future<bool> savePendingLocalBlePairing(
    PendingLocalBlePairing pairing,
  ) async {
    if (_localDataSource?.isInitialized != true ||
        PendingLocalBlePairing.fromJson(pairing.toJson()) == null) {
      return false;
    }
    return _serializePendingLocalBleMutation(() async {
      try {
        final records = await loadPendingLocalBlePairings();
        final merged = mergePendingLocalBlePairingForSave(records, pairing);
        if (merged == null) return false;
        await _localDataSource!.saveSettingsValue(
          _pendingLocalBlePairingKey,
          encodePendingLocalBlePairings(merged),
        );
        return true;
      } catch (_) {
        return false;
      }
    });
  }

  Future<bool> clearPendingLocalBlePairing({
    required String serverScope,
    String? sessionId,
  }) async {
    if (_localDataSource?.isInitialized != true) return false;
    return _serializePendingLocalBleMutation(() async {
      try {
        final records = await loadPendingLocalBlePairings();
        PendingLocalBlePairing? existing;
        for (final record in records) {
          if (record.serverScope == serverScope) {
            existing = record;
            break;
          }
        }
        if (sessionId != null &&
            existing != null &&
            existing.sessionId != sessionId) {
          return false;
        }
        if (existing == null) return true;
        records.removeWhere((record) => record.serverScope == serverScope);
        if (records.isEmpty) {
          await _localDataSource!
              .deleteSettingsValue(_pendingLocalBlePairingKey);
        } else {
          await _localDataSource!.saveSettingsValue(
            _pendingLocalBlePairingKey,
            encodePendingLocalBlePairings(records),
          );
        }
        return true;
      } catch (_) {
        // The unresolved pointer remains available for a future retry.
        return false;
      }
    });
  }

  Future<T> _serializePendingLocalBleMutation<T>(
    Future<T> Function() mutation,
  ) {
    final result = _pendingLocalBleMutationTail.then((_) => mutation());
    _pendingLocalBleMutationTail = result.then<void>(
      (_) {},
      onError: (Object _, StackTrace __) {},
    );
    return result;
  }

  // ============================================================
  // Room Page Layout
  // ============================================================

  /// Get room page layout as ordered lists of room IDs per page.
  List<List<String>>? getRoomPageLayout({String? scopeKey}) {
    final json = _roomPageLayoutJson(scopeKey: scopeKey);
    return _decodeRoomPageLayout(json);
  }

  /// Get the legacy unscoped room page layout.
  List<List<String>>? getLegacyRoomPageLayout() {
    return _decodeRoomPageLayout(_settings.roomPageAssignmentsJson);
  }

  /// Save room page layout.
  Future<void> saveRoomPageLayout(List<List<String>> pages,
      {String? scopeKey}) async {
    final json = jsonEncode(pages);
    if (scopeKey == null) {
      _settings = _settings.copyWith(roomPageAssignmentsJson: json);
      await _save();
      return;
    }

    await _localDataSource!.saveSettingsValue(
      _roomPageLayoutStorageKey(scopeKey),
      json,
    );
  }

  /// Clear room page layout.
  Future<void> clearRoomPageAssignments({String? scopeKey}) async {
    if (scopeKey == null) {
      _settings = _settings.clearField(clearRoomPageAssignmentsJson: true);
      await _save();
      return;
    }

    await _localDataSource!.deleteSettingsValue(
      _roomPageLayoutStorageKey(scopeKey),
    );
  }

  /// Copy the legacy global room page layout into a scoped slot.
  ///
  /// This is a one-time bridge from the previous single-layout storage model.
  Future<void> migrateLegacyRoomPageLayoutToScope(String scopeKey) async {
    final legacyJson = _settings.roomPageAssignmentsJson;
    if (legacyJson == null) return;

    final scopedKey = _roomPageLayoutStorageKey(scopeKey);
    final existing = _localDataSource!.getSettingsValue(scopedKey);
    if (existing is String && existing.isNotEmpty) {
      return;
    }

    await _localDataSource!.saveSettingsValue(scopedKey, legacyJson);
    _settings = _settings.clearField(clearRoomPageAssignmentsJson: true);
    await _save();
  }

  String _roomPageLayoutStorageKey(String scopeKey) {
    return '$roomPageLayoutScopePrefix$scopeKey';
  }

  /// Whether this account has a user-authored layout that has not reached the
  /// account cloud bundle yet.
  bool isRoomPageLayoutCloudDirty({
    required String userId,
    String? scopeKey,
  }) {
    return _localDataSource!.getSettingsValue(
          _roomPageLayoutCloudDirtyKey(userId, scopeKey),
        ) ==
        true;
  }

  /// Move a pending cloud-sync marker from an endpoint-keyed layout scope to
  /// its durable server scope before cloud restore can overwrite local edits.
  Future<bool> migrateRoomPageLayoutCloudDirty({
    required String userId,
    required String? scopeKey,
    Iterable<String> scopeKeyAliases = const <String>[],
  }) async {
    if (isRoomPageLayoutCloudDirty(userId: userId, scopeKey: scopeKey)) {
      return true;
    }

    final dirtyAliases = scopeKeyAliases
        .where(
          (alias) => isRoomPageLayoutCloudDirty(
            userId: userId,
            scopeKey: alias,
          ),
        )
        .toList(growable: false);
    if (dirtyAliases.isEmpty) return false;

    await setRoomPageLayoutCloudDirty(
      userId: userId,
      scopeKey: scopeKey,
      dirty: true,
    );
    for (final alias in dirtyAliases) {
      await setRoomPageLayoutCloudDirty(
        userId: userId,
        scopeKey: alias,
        dirty: false,
      );
    }
    return true;
  }

  /// Keep failed layout uploads authoritative across restart without changing
  /// the existing page-layout or cloud-bundle schema.
  Future<void> setRoomPageLayoutCloudDirty({
    required String userId,
    required bool dirty,
    String? scopeKey,
  }) async {
    final key = _roomPageLayoutCloudDirtyKey(userId, scopeKey);
    if (dirty) {
      await _localDataSource!.saveSettingsValue(key, true);
      return;
    }
    await _localDataSource!.deleteSettingsValue(key);
  }

  String _roomPageLayoutCloudDirtyKey(String userId, String? scopeKey) {
    final encodedUser = Uri.encodeComponent(userId);
    final encodedScope = Uri.encodeComponent(scopeKey ?? 'unscoped');
    return '$_roomPageLayoutCloudDirtyPrefix$encodedUser::$encodedScope';
  }

  /// Export app settings that should roam with a signed-in account.
  ///
  /// Keep this deliberately narrow for now. Device-local preferences stay
  /// local; the user-facing setting we sync is the All Rooms page layout.
  Map<String, dynamic> buildCloudSettingsBundle({
    String? roomLayoutScopeKey,
    String? roomLayoutHubKey,
    Iterable<String> roomLayoutHubKeyAliases = const <String>[],
  }) {
    final bundle = <String, dynamic>{
      'schema_version': 1,
    };

    final scopedPages = getRoomPageLayout(scopeKey: roomLayoutScopeKey);
    final legacyPages = getLegacyRoomPageLayout();
    final pages = scopedPages ?? legacyPages;
    if (pages != null) {
      bundle['all_rooms_layouts'] = <Map<String, dynamic>>[
        <String, dynamic>{
          if (roomLayoutHubKey != null) 'hub_key': roomLayoutHubKey,
          if (roomLayoutHubKeyAliases.isNotEmpty)
            'hub_key_aliases': roomLayoutHubKeyAliases.toList(growable: false),
          if (roomLayoutScopeKey != null)
            'source_scope_key': roomLayoutScopeKey,
          'pages': pages,
        },
      ];
    }

    return bundle;
  }

  /// Apply cloud-backed app settings to the current device.
  ///
  /// Restores the saved All Rooms page layout into the caller-provided current
  /// layout scope, so a backup captured on one local home/server ID can still
  /// apply to the equivalent server on this device.
  Future<bool> applyCloudSettingsBundle(
    Map<String, dynamic> bundle, {
    String? roomLayoutScopeKey,
    String? roomLayoutHubKey,
    Iterable<String> roomLayoutHubKeyAliases = const <String>[],
    bool overwrite = true,
  }) async {
    if (!overwrite && getRoomPageLayout(scopeKey: roomLayoutScopeKey) != null) {
      return false;
    }

    final layout = _findAllRoomsLayoutForHub(
      bundle,
      roomLayoutHubKey: roomLayoutHubKey,
      roomLayoutHubKeyAliases: roomLayoutHubKeyAliases,
    );
    if (layout == null) return false;

    final pages = _decodeRoomPageLayoutFromValue(layout['pages']);
    if (pages == null) return false;

    await saveRoomPageLayout(pages, scopeKey: roomLayoutScopeKey);
    return true;
  }

  static Map<dynamic, dynamic>? _findAllRoomsLayoutForHub(
    Map<String, dynamic> bundle, {
    String? roomLayoutHubKey,
    Iterable<String> roomLayoutHubKeyAliases = const <String>[],
  }) {
    final acceptedHubKeys = <String>{
      if (roomLayoutHubKey != null) roomLayoutHubKey,
      ...roomLayoutHubKeyAliases,
    };
    final layouts = bundle['all_rooms_layouts'];
    if (layouts is List) {
      for (final layout in layouts) {
        if (layout is! Map) continue;
        final layoutHubKeys = <String>{
          if (layout['hub_key'] is String) layout['hub_key'] as String,
          if (layout['hub_key_aliases'] is List)
            ...(layout['hub_key_aliases'] as List).whereType<String>(),
        };
        if (acceptedHubKeys.isEmpty ||
            acceptedHubKeys.any(layoutHubKeys.contains)) {
          return layout;
        }
      }
    }

    // Backward compatibility with early local snapshots before layouts were
    // keyed by hub.
    final legacyLayout = bundle['all_rooms_layout'];
    return legacyLayout is Map ? legacyLayout : null;
  }

  @visibleForTesting
  static Map<dynamic, dynamic>? findCloudRoomLayoutForTesting(
    Map<String, dynamic> bundle, {
    String? roomLayoutHubKey,
    Iterable<String> roomLayoutHubKeyAliases = const <String>[],
  }) {
    return _findAllRoomsLayoutForHub(
      bundle,
      roomLayoutHubKey: roomLayoutHubKey,
      roomLayoutHubKeyAliases: roomLayoutHubKeyAliases,
    );
  }

  String? _roomPageLayoutJson({String? scopeKey}) {
    if (scopeKey == null) {
      return _settings.roomPageAssignmentsJson;
    }
    final value = _localDataSource!.getSettingsValue(
      _roomPageLayoutStorageKey(scopeKey),
    );
    if (value is String) return value;
    return _findHomeScopedRoomPageLayoutJson(scopeKey);
  }

  String? _findHomeScopedRoomPageLayoutJson(String scopeKey) {
    for (final key in _localDataSource!.getSettingsKeysWithPrefix(
      roomPageLayoutScopePrefix,
    )) {
      if (!key.endsWith(':$scopeKey')) continue;
      final value = _localDataSource!.getSettingsValue(key);
      if (value is String && value.isNotEmpty) {
        return value;
      }
    }
    return null;
  }

  List<List<String>>? _decodeRoomPageLayout(String? json) {
    if (json == null) return null;
    try {
      return _decodeRoomPageLayoutFromValue(jsonDecode(json));
    } catch (e) {
      return null;
    }
  }

  List<List<String>>? _decodeRoomPageLayoutFromValue(Object? value) {
    if (value is! List) return null;
    try {
      return value
          .map((page) => (page as List<dynamic>).cast<String>().toList())
          .toList();
    } catch (e) {
      return null;
    }
  }

  // ============================================================
  // Time Formatting Helper
  // ============================================================

  /// Format a time in the given format.
  String formatTime(int hour, int minute, {required bool use24h}) {
    if (use24h) {
      return '${hour.toString().padLeft(2, '0')}:${minute.toString().padLeft(2, '0')}';
    }
    final period = hour >= 12 ? 'PM' : 'AM';
    final displayHour = hour == 0 ? 12 : (hour > 12 ? hour - 12 : hour);
    return '$displayHour:${minute.toString().padLeft(2, '0')} $period';
  }

  // ============================================================
  // Utility Methods
  // ============================================================

  /// Save current settings to storage.
  Future<void> _save() async {
    if (_localDataSource != null) {
      await _localDataSource!.saveSettings(_settings);
    }
  }

  /// Reset all settings to defaults.
  Future<void> resetToDefaults() async {
    _settings = AppSettings.defaults();
    await _save();
  }

  /// Clear all settings (for sign out / account deletion).
  Future<void> clearAll() async {
    if (_localDataSource != null) {
      await _localDataSource!.clearSettings();
    }
    // Clear SharedPreferences to prevent re-migration of stale data
    try {
      final prefs = await SharedPreferences.getInstance();
      await prefs.clear();
    } catch (_) {}
    _settings = AppSettings.defaults();
    _initialized = false;
  }
}
