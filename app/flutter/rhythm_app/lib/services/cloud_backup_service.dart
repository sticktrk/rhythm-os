import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:supabase_flutter/supabase_flutter.dart';

import '../backend/backend.dart';
import '../backend/auth/supabase_auth_backend.dart';
import 'auth_service.dart';
import 'server_bundle_api.dart';

/// Single cloud snapshot for the signed-in user.
///
/// We intentionally keep this to one row per user for now. The row carries
/// source-hub metadata so future UI can show where the latest snapshot came
/// from without making hub identity the primary key.
class CloudBackupSnapshot {
  const CloudBackupSnapshot({
    required this.userId,
    required this.sourceHubId,
    required this.sourceHubType,
    required this.sourceHubName,
    required this.sourceHubHost,
    required this.sourceHubPort,
    required this.configurationBundle,
    required this.backupBundle,
    this.homeId,
    this.homeName,
    this.capturedAt,
    this.updatedAt,
  });

  final String userId;
  final String sourceHubId;
  final String sourceHubType;
  final String sourceHubName;
  final String sourceHubHost;
  final int sourceHubPort;
  final String? homeId;
  final String? homeName;
  final Map<String, dynamic> configurationBundle;
  final Map<String, dynamic> backupBundle;
  final DateTime? capturedAt;
  final DateTime? updatedAt;

  factory CloudBackupSnapshot.fromRow(Map<String, dynamic> row) {
    return CloudBackupSnapshot(
      userId: row['user_id'] as String? ?? '',
      sourceHubId: row['source_hub_id'] as String? ?? '',
      sourceHubType: row['source_hub_type'] as String? ?? '',
      sourceHubName: row['source_hub_name'] as String? ?? '',
      sourceHubHost: row['source_hub_host'] as String? ?? '',
      sourceHubPort: (row['source_hub_port'] as num?)?.toInt() ?? 0,
      homeId: row['home_id'] as String?,
      homeName: row['home_name'] as String?,
      configurationBundle: Map<String, dynamic>.from(
        (row['configuration_bundle'] as Map?) ?? const <String, dynamic>{},
      ),
      backupBundle: Map<String, dynamic>.from(
        (row['backup_bundle'] as Map?) ?? const <String, dynamic>{},
      ),
      capturedAt: _parseDateTime(row['captured_at']),
      updatedAt: _parseDateTime(row['updated_at']),
    );
  }

  Map<String, dynamic> toUpsertJson() {
    return {
      'user_id': userId,
      'source_hub_id': sourceHubId,
      'source_hub_type': sourceHubType,
      'source_hub_name': sourceHubName,
      'source_hub_host': sourceHubHost,
      'source_hub_port': sourceHubPort,
      if (homeId != null) 'home_id': homeId,
      if (homeName != null) 'home_name': homeName,
      'configuration_bundle': configurationBundle,
      'backup_bundle': backupBundle,
      'captured_at': (capturedAt ?? DateTime.now()).toUtc().toIso8601String(),
    };
  }
}

DateTime? _parseDateTime(dynamic value) {
  if (value is String && value.isNotEmpty) {
    return DateTime.tryParse(value);
  }
  return null;
}

class CloudBackupService {
  CloudBackupService._();

  static const String tableName = 'user_cloud_snapshots';

  static CloudBackupService? _instance;
  static CloudBackupService get instance =>
      _instance ??= CloudBackupService._();

  final Map<String, Timer> _captureTimers = <String, Timer>{};
  final Set<String> _capturesInFlight = <String>{};
  final Set<String> _capturesQueued = <String>{};

  String? get _currentUserId => AuthService().currentUserId;

  SupabaseClient? get _client {
    if (!BackendProvider.isInitialized) return null;
    final auth = BackendProvider.instance.auth;
    if (auth is SupabaseAuthBackend) {
      return auth.client;
    }
    return null;
  }

  bool get canUseCloudBackups {
    final client = _client;
    if (client == null) return false;
    final auth = AuthService();
    return auth.isSignedIn && !auth.isAnonymous && auth.currentUserId != null;
  }

  void scheduleCapture({
    required Hub serverHub,
    Home? home,
    Duration delay = const Duration(seconds: 2),
    String reason = 'unspecified',
  }) {
    if (!canUseCloudBackups || serverHub.type != HubType.server) {
      return;
    }

    final opKey = _operationKey(serverHub);
    _captureTimers.remove(opKey)?.cancel();
    _captureTimers[opKey] = Timer(delay, () {
      _captureTimers.remove(opKey);
      unawaited(
        captureNow(
          serverHub: serverHub,
          home: home,
          reason: reason,
        ),
      );
    });
  }

  Future<CloudBackupSnapshot?> captureNow({
    required Hub serverHub,
    Home? home,
    String reason = 'manual',
  }) async {
    if (!canUseCloudBackups || serverHub.type != HubType.server) {
      return null;
    }

    final userId = _currentUserId;
    final client = _client;
    if (userId == null || client == null) return null;

    final opKey = _operationKey(serverHub);
    if (_capturesInFlight.contains(opKey)) {
      _capturesQueued.add(opKey);
      return null;
    }

    _capturesInFlight.add(opKey);
    try {
      final api = ServerBundleApi(endpoint: serverHub.endpoint);
      final configurationBundle = await api.getConfigurationBundle();
      final backupBundle = await api.getBackupBundle(includeSecrets: true);
      final snapshot = buildSnapshot(
        userId: userId,
        serverHub: serverHub,
        home: home,
        configurationBundle: configurationBundle,
        backupBundle: backupBundle,
      );

      await client.from(tableName).upsert(
            snapshot.toUpsertJson(),
            onConflict: 'user_id',
          );

      debugPrint(
        'CloudBackupService: captured snapshot for user=$userId '
        'hub=${serverHub.id} reason=$reason',
      );
      return snapshot;
    } catch (error, stackTrace) {
      debugPrint(
        'CloudBackupService: capture failed for hub=${serverHub.id} '
        'reason=$reason error=$error',
      );
      debugPrint('$stackTrace');
      return null;
    } finally {
      _capturesInFlight.remove(opKey);
      if (_capturesQueued.remove(opKey)) {
        scheduleCapture(
          serverHub: serverHub,
          home: home,
          delay: const Duration(seconds: 2),
          reason: 'queued_follow_up',
        );
      }
    }
  }

  Future<CloudBackupSnapshot?> getSnapshotForCurrentUser() async {
    if (!canUseCloudBackups) return null;
    final userId = _currentUserId;
    final client = _client;
    if (userId == null || client == null) return null;

    final row = await client
        .from(tableName)
        .select()
        .eq('user_id', userId)
        .maybeSingle();

    if (row is! Map<String, dynamic>) return null;
    return CloudBackupSnapshot.fromRow(row);
  }

  @visibleForTesting
  static CloudBackupSnapshot buildSnapshot({
    required String userId,
    required Hub serverHub,
    Home? home,
    required Map<String, dynamic> configurationBundle,
    required Map<String, dynamic> backupBundle,
    DateTime? capturedAt,
  }) {
    return CloudBackupSnapshot(
      userId: userId,
      sourceHubId: serverHub.id,
      sourceHubType: serverHub.type.name,
      sourceHubName: serverHub.name,
      sourceHubHost: serverHub.endpoint.host,
      sourceHubPort: serverHub.endpoint.port,
      homeId: home?.id,
      homeName: home?.name,
      configurationBundle: configurationBundle,
      backupBundle: backupBundle,
      capturedAt: capturedAt ?? DateTime.now(),
    );
  }

  @visibleForTesting
  void resetForTest() {
    for (final timer in _captureTimers.values) {
      timer.cancel();
    }
    _captureTimers.clear();
    _capturesInFlight.clear();
    _capturesQueued.clear();
  }

  String _operationKey(Hub serverHub) {
    return '${_currentUserId ?? 'no-user'}:${serverHub.id}';
  }
}
