import 'dart:async';

import 'package:dio/dio.dart';
import 'package:flutter/foundation.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:supabase_flutter/supabase_flutter.dart';

import '../backend/backend.dart';
import '../providers/room_page_provider.dart';
import 'account_cloud_sync_service.dart';
import 'auth_service.dart';
import 'employee_mode_service.dart';
import 'server_endpoint_resolver.dart';
import 'settings_service.dart';

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
    required this.appSettingsBundle,
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
  final Map<String, dynamic> appSettingsBundle;
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
      appSettingsBundle: Map<String, dynamic>.from(
        (row['app_settings_bundle'] as Map?) ?? const <String, dynamic>{},
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
      'app_settings_bundle': appSettingsBundle,
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

class CloudBackupCaptureException implements Exception {
  const CloudBackupCaptureException(this.message, {this.cause});

  final String message;
  final Object? cause;

  @override
  String toString() => message;
}

class CloudBackupService {
  CloudBackupService._();

  static const String tableName = 'user_cloud_snapshots';

  static CloudBackupService? _instance;
  static CloudBackupService get instance =>
      _instance ??= CloudBackupService._();

  final Map<String, Timer> _captureTimers = <String, Timer>{};
  final Map<String, Future<CloudBackupSnapshot>> _capturesInFlight =
      <String, Future<CloudBackupSnapshot>>{};
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
    if (EmployeeModeService.instance.isActive) return false;
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
        _captureNowSilently(
          serverHub: serverHub,
          home: home,
          reason: reason,
        ),
      );
    });
  }

  Future<CloudBackupSnapshot> captureNow({
    required Hub serverHub,
    Home? home,
    String reason = 'manual',
  }) async {
    if (!canUseCloudBackups || serverHub.type != HubType.server) {
      throw const CloudBackupCaptureException(
        'Cloud backups are not available right now.',
      );
    }

    final userId = _currentUserId;
    final client = _client;
    if (userId == null || client == null) {
      throw const CloudBackupCaptureException(
        'Cloud backups are not available right now.',
      );
    }

    await AccountCloudSyncService.instance.syncHomeAndServerHubs(
      home: home,
      hubs: [serverHub],
      reason: 'cloud_backup_capture',
    );

    final opKey = _operationKey(serverHub);
    final inFlight = _capturesInFlight[opKey];
    if (inFlight != null) {
      _capturesQueued.add(opKey);
      return inFlight;
    }

    final captureFuture = _captureNowInternal(
      serverHub: serverHub,
      home: home,
      userId: userId,
      client: client,
      reason: reason,
    );
    _capturesInFlight[opKey] = captureFuture;

    try {
      return await captureFuture;
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
    Map<String, dynamic> appSettingsBundle = const <String, dynamic>{},
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
      appSettingsBundle: appSettingsBundle,
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

  Future<void> _captureNowSilently({
    required Hub serverHub,
    Home? home,
    required String reason,
  }) async {
    try {
      await captureNow(
        serverHub: serverHub,
        home: home,
        reason: reason,
      );
    } catch (error, stackTrace) {
      debugPrint(
        'CloudBackupService: scheduled capture failed for hub=${serverHub.id} '
        'reason=$reason error=$error',
      );
      debugPrint('$stackTrace');
    }
  }

  Future<CloudBackupSnapshot> _captureNowInternal({
    required Hub serverHub,
    required String userId,
    required SupabaseClient client,
    required String reason,
    Home? home,
  }) async {
    final resolved = await ServerEndpointResolver.resolve(serverHub);
    final api = resolved.bundleApi();

    // Persist the secret-bearing GET /api/backup payload. The PUT response is
    // intentionally redacted and must not replace the stored backup.
    final backupBundle = await _runCaptureStep(
      serverHub: serverHub,
      reason: reason,
      context: 'Failed to fetch backup from the server.',
      action: () => api.getBackupBundle(includeSecrets: true),
    );

    final configurationBundle = await _getConfigurationBundleBestEffort(
      api: api,
      serverHub: serverHub,
      reason: reason,
    );
    final roomLayoutScopeKey = RoomPageProvider.layoutScopeFor(
      home: home,
      hubs: <Hub>[serverHub],
    );
    final appSettingsBundle = await _buildAppSettingsBundleForCapture(
      client: client,
      userId: userId,
      serverHub: serverHub,
      roomLayoutScopeKey: roomLayoutScopeKey,
    );

    final snapshot = buildSnapshot(
      userId: userId,
      serverHub: serverHub,
      home: home,
      configurationBundle: configurationBundle,
      backupBundle: backupBundle,
      appSettingsBundle: appSettingsBundle,
    );

    await _runCaptureStep(
      serverHub: serverHub,
      reason: reason,
      context: 'Failed to save the backup to cloud storage.',
      action: () => client.from(tableName).upsert(
            snapshot.toUpsertJson(),
            onConflict: 'user_id',
          ),
    );

    debugPrint(
      'CloudBackupService: captured snapshot for user=$userId '
      'hub=${serverHub.id} reason=$reason',
    );
    return snapshot;
  }

  Future<Map<String, dynamic>> _buildAppSettingsBundleForCapture({
    required SupabaseClient client,
    required String userId,
    required Hub serverHub,
    required String? roomLayoutScopeKey,
  }) async {
    final bundle = SettingsService.instance.buildCloudSettingsBundle(
      roomLayoutScopeKey: roomLayoutScopeKey,
      roomLayoutHubKey: RoomPageProvider.hubLayoutKey(serverHub),
    );
    if (bundle['all_rooms_layouts'] != null) {
      return bundle;
    }

    try {
      final row = await client
          .from(tableName)
          .select('app_settings_bundle')
          .eq('user_id', userId)
          .maybeSingle();
      final existingBundle = row?['app_settings_bundle'];
      if (existingBundle is Map) {
        return Map<String, dynamic>.from(existingBundle);
      }
    } catch (error) {
      debugPrint(
        'CloudBackupService: existing app settings lookup failed: $error',
      );
    }

    return bundle;
  }

  Future<Map<String, dynamic>> _getConfigurationBundleBestEffort({
    required RhythmBundleApi api,
    required Hub serverHub,
    required String reason,
  }) async {
    try {
      return await api.getConfigurationBundle();
    } catch (error, stackTrace) {
      final message = _buildCaptureErrorMessage(
        'Configuration metadata was unavailable; continuing with backup only.',
        error,
      );
      debugPrint(
        'CloudBackupService: optional configuration fetch failed '
        'for hub=${serverHub.id} reason=$reason error=$message',
      );
      debugPrint('$stackTrace');
      return const <String, dynamic>{};
    }
  }

  Future<T> _runCaptureStep<T>({
    required Hub serverHub,
    required String reason,
    required String context,
    required Future<T> Function() action,
  }) async {
    try {
      return await action();
    } catch (error, stackTrace) {
      final message = _buildCaptureErrorMessage(context, error);
      debugPrint(
        'CloudBackupService: capture failed for hub=${serverHub.id} '
        'reason=$reason error=$message',
      );
      debugPrint('$stackTrace');
      throw CloudBackupCaptureException(message, cause: error);
    }
  }

  String _buildCaptureErrorMessage(String context, Object error) {
    final detail = _formatCaptureErrorDetail(error);
    if (detail == null || detail.isEmpty) return context;
    return '$context $detail';
  }

  String? _formatCaptureErrorDetail(Object error) {
    if (error is CloudBackupCaptureException) {
      return error.message;
    }

    if (error is RhythmApiException) {
      final parts = <String>[
        if (error.statusCode != null) 'HTTP ${error.statusCode}',
        if (error.serverMessage != null &&
            error.serverMessage!.trim().isNotEmpty)
          error.serverMessage!.trim(),
        if ((error.serverMessage == null ||
                error.serverMessage!.trim().isEmpty) &&
            error.message.trim().isNotEmpty)
          error.message.trim(),
      ];
      return parts.isEmpty ? null : parts.join(' ');
    }

    if (error is DioException) {
      final response = error.response;
      if (response != null) {
        final status = response.statusCode;
        final body = response.data?.toString().trim();
        if (body != null && body.isNotEmpty) {
          return '(HTTP $status) $body';
        }
        if (status != null) {
          return '(HTTP $status)';
        }
      }

      return switch (error.type) {
        DioExceptionType.connectionTimeout ||
        DioExceptionType.receiveTimeout ||
        DioExceptionType.sendTimeout =>
          'The request timed out.',
        DioExceptionType.connectionError => 'Could not reach the server.',
        _ => error.message,
      };
    }

    final message = error.toString().trim();
    if (message.isEmpty) return null;
    return message.startsWith('Exception: ')
        ? message.substring('Exception: '.length)
        : message;
  }
}
