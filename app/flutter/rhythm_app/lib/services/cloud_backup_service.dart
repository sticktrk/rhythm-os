import 'dart:async';

import 'package:dio/dio.dart';
import 'package:flutter/foundation.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:supabase_flutter/supabase_flutter.dart';

import '../backend/backend.dart';
import '../providers/room_page_provider.dart';
import 'account_cloud_sync_service.dart';
import 'analytics_service.dart';
import 'auth_service.dart';
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

  /// Portable snapshots never contain HA device/fabric state or credentials.
  bool get hasApplianceBackup =>
      backupBundle.isNotEmpty &&
      backupBundle['kind'] != 'rhythm_portable_snapshot';

  /// Keep both migration sources available when an add-on starts synchronizing
  /// before the owner has transferred the old appliance's settings.
  List<CloudBackupSnapshot> get lightingRestoreSources {
    final portable = appSettingsBundle['portable_lighting_snapshot'];
    return [
      this,
      if (hasApplianceBackup && portable is Map)
        CloudBackupSnapshot.fromRow({
          ...Map<String, dynamic>.from(portable),
          'user_id': userId,
          'backup_bundle': {
            'kind': 'rhythm_portable_snapshot',
            'schema_version': 1,
          },
          'app_settings_bundle': appSettingsBundle,
        }),
    ];
  }

  Map<String, dynamic> get lightingRestorePayload =>
      hasApplianceBackup ? backupBundle : configurationBundle;

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
  final Map<String, Timer> _appSettingsSyncTimers = <String, Timer>{};
  final Map<String, Future<void>> _appSettingsSyncTails =
      <String, Future<void>>{};
  final Map<String, int> _appSettingsSyncGenerations = <String, int>{};

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
        _captureNowSilently(
          serverHub: serverHub,
          home: home,
          reason: reason,
        ),
      );
    });
  }

  /// Debounce a user-authored All Rooms layout into the signed-in account's
  /// existing app-settings bundle without fetching an appliance backup.
  void scheduleAppSettingsSync({
    required Hub serverHub,
    required String? roomLayoutScopeKey,
    Home? home,
    Duration delay = const Duration(milliseconds: 600),
    String reason = 'room_layout_changed',
  }) {
    final userId = _currentUserId;
    if (!canUseCloudBackups ||
        userId == null ||
        serverHub.type != HubType.server) {
      return;
    }

    final opKey = '$userId:${RoomPageProvider.hubLayoutKey(serverHub)}';
    final generation = (_appSettingsSyncGenerations[opKey] ?? 0) + 1;
    _appSettingsSyncGenerations[opKey] = generation;
    unawaited(
      _markLayoutDirtyAndSchedule(
        opKey: opKey,
        generation: generation,
        userId: userId,
        serverHub: serverHub,
        home: home,
        roomLayoutScopeKey: roomLayoutScopeKey,
        delay: delay,
        reason: reason,
      ),
    );
  }

  Future<void> _markLayoutDirtyAndSchedule({
    required String opKey,
    required int generation,
    required String userId,
    required Hub serverHub,
    required Home? home,
    required String? roomLayoutScopeKey,
    required Duration delay,
    required String reason,
  }) async {
    await SettingsService.instance.setRoomPageLayoutCloudDirty(
      userId: userId,
      scopeKey: roomLayoutScopeKey,
      dirty: true,
    );
    if (_appSettingsSyncGenerations[opKey] != generation) return;

    _appSettingsSyncTimers.remove(opKey)?.cancel();
    _appSettingsSyncTimers[opKey] = Timer(delay, () {
      _appSettingsSyncTimers.remove(opKey);
      final previous = _appSettingsSyncTails[opKey] ?? Future<void>.value();
      final next = previous.then((_) async {
        await _syncAppSettingsSilently(
          opKey: opKey,
          generation: generation,
          expectedUserId: userId,
          serverHub: serverHub,
          home: home,
          roomLayoutScopeKey: roomLayoutScopeKey,
          reason: reason,
        );
      });
      late final Future<void> tail;
      tail = next.whenComplete(() {
        if (identical(_appSettingsSyncTails[opKey], tail)) {
          _appSettingsSyncTails.remove(opKey);
        }
      });
      _appSettingsSyncTails[opKey] = tail;
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

  Future<void> _syncAppSettingsSilently({
    required String opKey,
    required int generation,
    required String expectedUserId,
    required Hub serverHub,
    required Home? home,
    required String? roomLayoutScopeKey,
    required String reason,
  }) async {
    if (_currentUserId != expectedUserId) return;
    var outcome = 'failed';
    try {
      final synced = await _syncAppSettingsNow(
        expectedUserId: expectedUserId,
        serverHub: serverHub,
        home: home,
        roomLayoutScopeKey: roomLayoutScopeKey,
        reason: reason,
      );
      if (!synced) return;
      outcome = 'succeeded';
      if (_appSettingsSyncGenerations[opKey] == generation) {
        await SettingsService.instance.setRoomPageLayoutCloudDirty(
          userId: expectedUserId,
          scopeKey: roomLayoutScopeKey,
          dirty: false,
        );
      }
    } catch (error, stackTrace) {
      debugPrint('CloudBackupService: app settings sync failed: $error');
      debugPrint('$stackTrace');
    } finally {
      final pages = SettingsService.instance.getRoomPageLayout(
            scopeKey: roomLayoutScopeKey,
          ) ??
          const <List<String>>[];
      unawaited(
        AnalyticsService().logRoomLayoutCloudSyncCompleted(
          direction: 'upload',
          outcome: outcome,
          pageCount: pages.length,
          roomCount: pages.expand((page) => page).toSet().length,
        ),
      );
    }
  }

  Future<bool> _syncAppSettingsNow({
    required String expectedUserId,
    required Hub serverHub,
    required Home? home,
    required String? roomLayoutScopeKey,
    required String reason,
  }) async {
    final client = _client;
    if (!canUseCloudBackups ||
        client == null ||
        _currentUserId != expectedUserId) {
      return false;
    }

    final localBundle = SettingsService.instance.buildCloudSettingsBundle(
      roomLayoutScopeKey: roomLayoutScopeKey,
      roomLayoutHubKey: RoomPageProvider.hubLayoutKey(serverHub),
      roomLayoutHubKeyAliases: RoomPageProvider.hubLayoutKeyAliases(serverHub),
    );
    if (localBundle['all_rooms_layouts'] == null) return false;

    var row = await client
        .from(tableName)
        .select('app_settings_bundle')
        .eq('user_id', expectedUserId)
        .maybeSingle();
    if (row == null) {
      // The capture path records either an appliance backup or a distinctly
      // marked portable snapshot, along with the account layout.
      await captureNow(
        serverHub: serverHub,
        home: home,
        reason: '${reason}_initial_snapshot',
      );
      if (_currentUserId != expectedUserId) return false;
      // A capture already in flight may have built its settings bundle before
      // this edit. Read the created row and apply the exact debounced layout
      // before considering the dirty marker delivered.
      row = await client
          .from(tableName)
          .select('app_settings_bundle')
          .eq('user_id', expectedUserId)
          .maybeSingle();
      if (row == null) return false;
    }

    await persistAppSettings(
      client: client,
      userId: expectedUserId,
      replacement: localBundle,
    );
    debugPrint(
      'CloudBackupService: synced account All Rooms layout reason=$reason',
    );
    return true;
  }

  @visibleForTesting
  static Map<String, dynamic> mergeAppSettingsBundles(
    Map<String, dynamic> existing,
    Map<String, dynamic> replacement,
  ) {
    final merged = Map<String, dynamic>.from(existing);
    merged['schema_version'] =
        replacement['schema_version'] ?? existing['schema_version'] ?? 1;
    if (replacement['portable_lighting_snapshot'] is Map) {
      merged['portable_lighting_snapshot'] =
          replacement['portable_lighting_snapshot'];
    }
    final existingSourceKeys = existing['lighting_source_hub_keys'];
    final replacementSourceKeys = replacement['lighting_source_hub_keys'];
    if (replacementSourceKeys is Map) {
      merged['lighting_source_hub_keys'] = {
        if (existingSourceKeys is Map) ...existingSourceKeys,
        ...replacementSourceKeys,
      };
    }

    final replacementLayouts = _layoutMaps(replacement['all_rooms_layouts']);
    if (replacementLayouts.isEmpty) return merged;
    final replacementKeys =
        replacementLayouts.expand(_layoutIdentityKeys).toSet();
    final retained = _layoutMaps(existing['all_rooms_layouts'])
        .where(
          (layout) => _layoutIdentityKeys(layout)
              .toSet()
              .intersection(replacementKeys)
              .isEmpty,
        )
        .toList(growable: true);
    retained.addAll(replacementLayouts);
    merged['all_rooms_layouts'] = retained;
    return merged;
  }

  static List<Map<String, dynamic>> _layoutMaps(Object? value) {
    if (value is! List) return <Map<String, dynamic>>[];
    return value
        .whereType<Map>()
        .map((layout) => Map<String, dynamic>.from(layout))
        .toList(growable: false);
  }

  static Iterable<String> _layoutIdentityKeys(
      Map<String, dynamic> layout) sync* {
    final primary = layout['hub_key'];
    if (primary is String && primary.isNotEmpty) yield primary;
    final aliases = layout['hub_key_aliases'];
    if (aliases is List) yield* aliases.whereType<String>();
  }

  /// A migration is an explicit transfer between installations. Only reviewed
  /// room IDs enter the destination layout; source aliases never follow them.
  static Map<String, dynamic> remapLightingLayout({
    required CloudBackupSnapshot source,
    required Hub targetHub,
    required Map<String, String> roomMappings,
  }) {
    final sourceHub = Hub.server(
      id: source.sourceHubId,
      homeId: source.homeId ?? '',
      name: source.sourceHubName,
      host: source.sourceHubHost,
      port: source.sourceHubPort,
    );
    final sourceKeys = {
      RoomPageProvider.hubLayoutKey(sourceHub),
      ...RoomPageProvider.hubLayoutKeyAliases(sourceHub),
    };
    final recordedKeys = source.appSettingsBundle['lighting_source_hub_keys'];
    if (recordedKeys is Map && recordedKeys[source.sourceHubId] is String) {
      sourceKeys.add(recordedKeys[source.sourceHubId] as String);
    }
    final layout = _layoutMaps(source.appSettingsBundle['all_rooms_layouts'])
        .where((entry) => _layoutIdentityKeys(entry).any(sourceKeys.contains))
        .firstOrNull;
    final rawPages = (layout ??
        (source.appSettingsBundle['all_rooms_layout'] is Map
            ? source.appSettingsBundle['all_rooms_layout'] as Map
            : const {}))['pages'];
    if (rawPages is! List) return const {};
    final assigned = <String>{};
    final pages = <List<String>>[
      for (final page in rawPages)
        if (page is List)
          [
            for (final id in page.whereType<String>())
              if (roomMappings[id] != null && assigned.add(roomMappings[id]!))
                roomMappings[id]!,
          ],
    ];
    if (assigned.isEmpty) return const {};
    return {
      'schema_version': 1,
      'all_rooms_layouts': [
        {
          'hub_key': RoomPageProvider.hubLayoutKey(targetHub),
          'hub_key_aliases': RoomPageProvider.hubLayoutKeyAliases(targetHub),
          'pages': pages,
        },
      ],
    };
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
      appSettingsBundle: {
        ...appSettingsBundle,
        'lighting_source_hub_keys': {
          if (appSettingsBundle['lighting_source_hub_keys'] is Map)
            ...(appSettingsBundle['lighting_source_hub_keys'] as Map),
          serverHub.id: RoomPageProvider.hubLayoutKey(serverHub),
        },
      },
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
    for (final timer in _appSettingsSyncTimers.values) {
      timer.cancel();
    }
    _appSettingsSyncTimers.clear();
    _appSettingsSyncTails.clear();
    _appSettingsSyncGenerations.clear();
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

  /// Query the actual endpoint before requesting any secret-bearing backup.
  /// Add-ons synchronize portable Rhythm profiles and phone preferences only;
  /// Home Assistant owns full-installation backup and recovery.
  @visibleForTesting
  static Future<
          ({Map<String, dynamic> backup, Map<String, dynamic> configuration})>
      captureSupportedBundles(RhythmBundleApi api) async {
    final deployment = await api.getDeploymentCapabilities();
    if (!deployment.fullBackupExport &&
        !deployment.portableProfiles &&
        !deployment.portableSettings) {
      throw const CloudBackupCaptureException(
        'This server does not support cloud configuration snapshots.',
      );
    }
    final backup = deployment.fullBackupExport
        ? await api.getBackupBundle(includeSecrets: true)
        : <String, dynamic>{
            'kind': 'rhythm_portable_snapshot',
            'schema_version': 1,
          };
    Map<String, dynamic> configuration = const {};
    if (deployment.portableSettings) {
      configuration = await api.getLightingSettings();
    } else if (deployment.portableProfiles) {
      try {
        configuration = await api.getConfigurationBundle();
      } catch (_) {
        // A portable snapshot with no profile data would falsely report success.
        if (!deployment.fullBackupExport) rethrow;
      }
    }
    return (backup: backup, configuration: configuration);
  }

  /// Add-on capture must not replace the account's appliance rollback material
  /// or relabel its source identity. Until portable profiles have dedicated
  /// storage, the latest add-on settings live alongside phone preferences. The
  /// original appliance row remains an independently selectable restore source.
  @visibleForTesting
  static Map<String, dynamic> portableSnapshotSettingsUpdate({
    required CloudBackupSnapshot existing,
    required CloudBackupSnapshot portable,
    bool overwriteSourceLayout = false,
  }) {
    if (portable.hasApplianceBackup) {
      throw ArgumentError('Expected a portable configuration snapshot.');
    }
    final appSettings = _captureAppSettingsForWrite(
      existing: existing.appSettingsBundle, snapshot: portable,
      overwriteSourceLayout: overwriteSourceLayout,
    );
    appSettings['portable_lighting_snapshot'] =
        _portableAppSettings(portable)['portable_lighting_snapshot'];
    return {
      if (existing.backupBundle['kind'] == 'rhythm_portable_snapshot')
        ...portable.toUpsertJson(),
      'app_settings_bundle': appSettings,
    };
  }

  /// Captures carry an account-wide view for recovery, but only this source's
  /// layout is their input. Reapply the dirty/layout decision against the latest
  /// row so an old capture cannot roll back another phone or hub's edits.
  static Map<String, dynamic> _captureAppSettingsForWrite({
    required Map<String, dynamic> existing,
    required CloudBackupSnapshot snapshot,
    required bool overwriteSourceLayout,
  }) {
    final sourceHub = Hub.server(
      id: snapshot.sourceHubId, homeId: snapshot.homeId ?? '',
      name: snapshot.sourceHubName, host: snapshot.sourceHubHost,
      port: snapshot.sourceHubPort,
    );
    final recorded = snapshot.appSettingsBundle['lighting_source_hub_keys'];
    final sourceKey = recorded is Map && recorded[snapshot.sourceHubId] is String
        ? recorded[snapshot.sourceHubId] as String
        : RoomPageProvider.hubLayoutKey(sourceHub);
    final keys = {sourceKey, RoomPageProvider.hubLayoutKey(sourceHub), snapshot.sourceHubId};
    final local = <String, dynamic>{
      'schema_version': snapshot.appSettingsBundle['schema_version'] ?? 1,
      'all_rooms_layouts': _layoutMaps(snapshot.appSettingsBundle['all_rooms_layouts'])
          .where((layout) => _layoutIdentityKeys(layout).any(keys.contains)).toList(),
    };
    final chosen = appSettingsBundleForCapture(
      existing: existing, local: local,
      hasUnsyncedLayoutEdit: overwriteSourceLayout,
    );
    return mergeAppSettingsBundles(chosen, {
      'lighting_source_hub_keys': {snapshot.sourceHubId: sourceKey},
    });
  }

  static Map<String, dynamic> _portableAppSettings(
    CloudBackupSnapshot snapshot,
  ) =>
      {
        ...snapshot.appSettingsBundle,
        'portable_lighting_snapshot': {
          'source_hub_id': snapshot.sourceHubId,
          'source_hub_type': snapshot.sourceHubType,
          'source_hub_name': snapshot.sourceHubName,
          'source_hub_host': snapshot.sourceHubHost,
          'source_hub_port': snapshot.sourceHubPort,
          'home_id': snapshot.homeId,
          'home_name': snapshot.homeName,
          'captured_at': snapshot.capturedAt?.toUtc().toIso8601String(),
          'configuration_bundle': snapshot.configurationBundle,
        },
      };

  /// Every writer compares the row version it merged. A concurrent capture or
  /// layout sync causes a reread, never an unconditional replacement of JSON.
  static Future<CloudBackupSnapshot> _persistCloudMutation({
    required SupabaseClient client,
    required String userId,
    CloudBackupSnapshot? initial,
    required Map<String, dynamic> Function(CloudBackupSnapshot) update,
  }) async {
    for (var attempt = 0; attempt < 5; attempt++) {
      final row = await client.from(tableName).select()
          .eq('user_id', userId).maybeSingle();
      if (row == null) {
        if (initial == null) throw StateError('Cloud backup no longer exists.');
        try {
          await client.from(tableName).insert(initial.toUpsertJson());
          return initial;
        } on PostgrestException catch (error) {
          if (error.code != '23505') rethrow;
          continue;
        }
      }
      final updatedAt = row['updated_at'];
      if (updatedAt is! String || updatedAt.isEmpty) {
        throw StateError('Cloud backup has no revision. Retry after refreshing.');
      }
      final saved = await client.from(tableName)
          .update(update(CloudBackupSnapshot.fromRow(row)))
          .eq('user_id', userId).eq('updated_at', updatedAt).select();
      if (saved.isNotEmpty) return CloudBackupSnapshot.fromRow(saved.single);
    }
    throw StateError('The cloud backup changed during saving. Please retry.');
  }

  @visibleForTesting
  static Future<CloudBackupSnapshot> persistAppSettings({
    required SupabaseClient client,
    required String userId,
    required Map<String, dynamic> replacement,
  }) => _persistCloudMutation(
    client: client, userId: userId,
    update: (existing) => {
      'app_settings_bundle': mergeAppSettingsBundles(existing.appSettingsBundle, replacement),
    },
  );

  @visibleForTesting
  static Future<CloudBackupSnapshot> persistPortableSnapshot({
    required SupabaseClient client,
    required CloudBackupSnapshot portable,
    bool overwriteSourceLayout = false,
  }) {
    if (portable.hasApplianceBackup) {
      throw ArgumentError('Expected a portable configuration snapshot.');
    }
    return _persistCloudMutation(
      client: client, userId: portable.userId, initial: portable,
      update: (existing) => portableSnapshotSettingsUpdate(
        existing: existing, portable: portable,
        overwriteSourceLayout: overwriteSourceLayout,
      ),
    );
  }

  @visibleForTesting
  static Future<CloudBackupSnapshot> persistApplianceSnapshot({
    required SupabaseClient client,
    required CloudBackupSnapshot appliance,
    bool overwriteSourceLayout = false,
  }) {
    if (!appliance.hasApplianceBackup) {
      throw ArgumentError('Expected an appliance backup.');
    }
    return _persistCloudMutation(
      client: client, userId: appliance.userId, initial: appliance,
      update: (existing) {
        final appSettings = _captureAppSettingsForWrite(
          existing: existing.appSettingsBundle, snapshot: appliance,
          overwriteSourceLayout: overwriteSourceLayout,
        );
        final latestPortable = existing.hasApplianceBackup
            ? existing.appSettingsBundle['portable_lighting_snapshot']
            : _portableAppSettings(existing)['portable_lighting_snapshot'];
        if (latestPortable is Map) {
          appSettings['portable_lighting_snapshot'] = latestPortable;
        }
        return {...appliance.toUpsertJson(), 'app_settings_bundle': appSettings};
      },
    );
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

    final bundles = await captureSupportedBundles(api);
    final backupBundle = bundles.backup;
    final configurationBundle = bundles.configuration;
    final roomLayoutScopeKey = RoomPageProvider.layoutScopeFor(
      home: home,
      hubs: <Hub>[serverHub],
    );
    final appSettings = await _buildAppSettingsBundleForCapture(
      client: client,
      userId: userId,
      serverHub: serverHub,
      home: home,
      roomLayoutScopeKey: roomLayoutScopeKey,
      reason: reason,
    );

    final snapshot = buildSnapshot(
      userId: userId,
      serverHub: serverHub,
      home: home,
      configurationBundle: configurationBundle,
      backupBundle: backupBundle,
      appSettingsBundle: appSettings.bundle,
    );

    var persistedSnapshot = snapshot;
    await _runCaptureStep(
      serverHub: serverHub,
      reason: reason,
      context: 'Failed to save the configuration to cloud storage.',
      action: () async {
        if (snapshot.hasApplianceBackup) {
          persistedSnapshot = await persistApplianceSnapshot(
            client: client, appliance: snapshot,
            overwriteSourceLayout: appSettings.overwriteSourceLayout,
          );
          return;
        }
        persistedSnapshot = await persistPortableSnapshot(
          client: client,
          portable: snapshot,
          overwriteSourceLayout: appSettings.overwriteSourceLayout,
        );
      },
    );

    debugPrint(
      'CloudBackupService: captured snapshot for user=$userId '
      'hub=${serverHub.id} reason=$reason',
    );
    return persistedSnapshot;
  }

  Future<({Map<String, dynamic> bundle, bool overwriteSourceLayout})>
      _buildAppSettingsBundleForCapture({
    required SupabaseClient client,
    required String userId,
    required Hub serverHub,
    required Home? home,
    required String? roomLayoutScopeKey,
    required String reason,
  }) async {
    final localBundle = SettingsService.instance.buildCloudSettingsBundle(
      roomLayoutScopeKey: roomLayoutScopeKey,
      roomLayoutHubKey: RoomPageProvider.hubLayoutKey(serverHub),
      roomLayoutHubKeyAliases: RoomPageProvider.hubLayoutKeyAliases(serverHub),
    );

    // A capture rewrites the whole app settings bundle, so it must start from
    // what the account already has. Failing the capture is safer than
    // overwriting another device's layout with a guess.
    final row = await _runCaptureStep(
      serverHub: serverHub,
      reason: reason,
      context: 'Failed to read the existing cloud app settings.',
      action: () => client
          .from(tableName)
          .select('app_settings_bundle')
          .eq('user_id', userId)
          .maybeSingle(),
    );
    final existingBundle = row?['app_settings_bundle'];
    final existing = existingBundle is Map
        ? Map<String, dynamic>.from(existingBundle)
        : null;

    final scopeKeys = <String?>{
      roomLayoutScopeKey,
      ...RoomPageProvider.layoutScopeAliasesFor(
        home: home,
        hubs: <Hub>[serverHub],
      ),
    };
    final hasUnsyncedLayoutEdit = scopeKeys.any(
      (scopeKey) => SettingsService.instance.isRoomPageLayoutCloudDirty(
        userId: userId,
        scopeKey: scopeKey,
      ),
    );

    return (
      bundle: appSettingsBundleForCapture(
        existing: existing,
        local: localBundle,
        hasUnsyncedLayoutEdit: hasUnsyncedLayoutEdit,
      ),
      overwriteSourceLayout: hasUnsyncedLayoutEdit,
    );
  }

  /// Choose the app settings bundle a backup capture writes.
  ///
  /// Only a user-authored layout edit that has not reached the account yet
  /// may replace a layout the account already holds. The local layout is
  /// otherwise frequently the automatic "everything on page 0" arrangement a
  /// device builds before it restores the account layout, and letting a
  /// routine capture publish that would collapse the layout on every other
  /// device at its next restore. When the account holds no layout for this
  /// hub, the local layout seeds it.
  @visibleForTesting
  static Map<String, dynamic> appSettingsBundleForCapture({
    required Map<String, dynamic>? existing,
    required Map<String, dynamic> local,
    required bool hasUnsyncedLayoutEdit,
  }) {
    if (existing == null) return local;
    final localLayouts = _layoutMaps(local['all_rooms_layouts']);
    if (localLayouts.isEmpty) return existing;
    if (hasUnsyncedLayoutEdit) {
      return mergeAppSettingsBundles(existing, local);
    }
    final localKeys = localLayouts.expand(_layoutIdentityKeys).toSet();
    final accountHasLayoutForHub = _layoutMaps(
      existing['all_rooms_layouts'],
    ).any((layout) => _layoutIdentityKeys(layout).any(localKeys.contains));
    if (accountHasLayoutForHub) return existing;
    return mergeAppSettingsBundles(existing, local);
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
