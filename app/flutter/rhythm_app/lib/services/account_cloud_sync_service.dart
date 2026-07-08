import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:supabase_flutter/supabase_flutter.dart';

import '../backend/backend.dart';
import 'account_data_encryption_service.dart';
import 'auth_service.dart';

/// A Home plus the Rhythm Server hubs saved for that Home in the account.
class AccountHomeServerHubs {
  const AccountHomeServerHubs({
    required this.home,
    required this.serverHubs,
  });

  final Home home;
  final List<Hub> serverHubs;

  bool get hasServerHub => serverHubs.isNotEmpty;
  bool get hasRemoteServerHub =>
      serverHubs.any((hub) => hub.remoteEndpoint != null);

  Hub? get preferredServerHub {
    if (serverHubs.isEmpty) return null;

    final enabled = serverHubs.where((hub) => hub.enabled).toList();
    final candidates =
        enabled.isNotEmpty ? enabled : List<Hub>.from(serverHubs);
    candidates.sort((left, right) {
      final leftRecency = left.lastConnected ?? left.updatedAt;
      final rightRecency = right.lastConnected ?? right.updatedAt;
      return rightRecency.compareTo(leftRecency);
    });
    return candidates.first;
  }
}

/// Keeps the signed-in account's minimal cloud home/server-hub records current.
///
/// Local-only and anonymous users are valid app users, but they are not cloud
/// account users. This service is intentionally a no-op until Supabase has a
/// non-anonymous session, which makes backup/restore and remote access share
/// the same account boundary.
class AccountCloudSyncService {
  AccountCloudSyncService._();

  static final AccountCloudSyncService instance = AccountCloudSyncService._();

  static const _serverHubColumns =
      'id,home_id,type,name,endpoint,enabled,token,encrypted_token,remote_endpoint,server_instance_id,last_connected,created_at,updated_at';
  static const _serverHubColumnsWithoutServerInstanceId =
      'id,home_id,type,name,endpoint,enabled,token,encrypted_token,remote_endpoint,last_connected,created_at,updated_at';
  static const _legacyServerHubColumnsWithServerInstanceId =
      'id,home_id,type,name,endpoint,enabled,token,remote_endpoint,server_instance_id,last_connected,created_at,updated_at';
  static const _legacyServerHubColumns =
      'id,home_id,type,name,endpoint,enabled,token,remote_endpoint,last_connected,created_at,updated_at';

  bool get canUseSignedInCloudFeatures {
    final auth = AuthService();
    return _client != null &&
        auth.isSignedIn &&
        !auth.isAnonymous &&
        auth.currentUserId != null;
  }

  static String hubTokenPurpose({
    required String homeId,
    required String hubId,
  }) {
    return 'hub-token:$homeId:$hubId';
  }

  Future<List<AccountHomeServerHubs>> loadHomesAndServerHubs() async {
    final client = _client;
    final userId = AuthService().currentUserId;
    if (!canUseSignedInCloudFeatures || client == null) {
      return const [];
    }

    final homeRows = await client
        .from('homes')
        .select(
          'id,name,owner_id,member_ids,location,sleep_schedule,curve_config,timezone,created_at,updated_at',
        )
        .order('updated_at', ascending: false);
    final homes = homeRows
        .whereType<Map>()
        .map((row) => Home.fromSupabase(_stringKeyMap(row)))
        .where((home) => accountHomeBelongsToUserForTesting(
              home: home,
              userId: userId,
            ))
        .toList(growable: false);
    if (homes.isEmpty) return const [];

    final homeIds = homes.map((home) => home.id).toList(growable: false);
    final hubRows = await _loadServerHubRows(client, homeIds);
    final hubs = <Hub>[];
    for (final row in hubRows.whereType<Map>()) {
      final mapped = _stringKeyMap(row);
      final decryptedToken = await _decryptHubToken(mapped);
      final legacyEncryptedToken =
          _encryptedEnvelopeFromLegacyToken(mapped['token']);
      final legacyToken =
          legacyEncryptedToken == null ? mapped['token'] as String? : null;
      final token = decryptedToken ?? legacyToken;
      hubs.add(
        Hub.fromSupabase({
          ...mapped,
          if (token != null && token.trim().isNotEmpty) 'token': token,
        }),
      );
    }

    return accountHomeServerHubsFromRowsForTesting(
      homes: homes,
      serverHubs: hubs,
    );
  }

  Future<void> syncHomeAndServerHubs({
    required Home? home,
    required Iterable<Hub> hubs,
    required String reason,
    Set<String> clearRemoteEndpointHubIds = const <String>{},
  }) async {
    final auth = AuthService();
    final userId = auth.currentUserId;
    final client = _client;
    if (client == null ||
        !auth.isSignedIn ||
        auth.isAnonymous ||
        userId == null ||
        home == null) {
      return;
    }

    final serverHubs = hubs
        .where((hub) => hub.type == HubType.server && hub.homeId == home.id)
        .toList(growable: false);
    if (!shouldSyncHomeAndServerHubsForTesting(
      home: home,
      serverHubs: serverHubs,
    )) {
      debugPrint(
        'AccountCloudSyncService: sync skipped for home=${home.id} '
        'reason=$reason server_hubs=0',
      );
      return;
    }
    if (!accountHomeCanSyncForUserForTesting(home: home, userId: userId)) {
      debugPrint(
        'AccountCloudSyncService: sync skipped for home=${home.id} '
        'reason=$reason not a member of this account',
      );
      return;
    }

    try {
      final identityConflict = await _findServerHubIdentityConflict(
        client,
        homeId: home.id,
        serverHubs: serverHubs,
      );
      if (identityConflict != null) {
        debugPrint(
          'AccountCloudSyncService: sync skipped for home=${home.id} '
          'reason=$reason server_instance_id='
          '${identityConflict.serverInstanceId} already exists as '
          'hub=${identityConflict.existingHubId}; local_hub='
          '${identityConflict.localHubId}',
        );
        return;
      }

      final homeAlreadyInCloud = await _accountHomeExists(client, home.id);

      await client.from('homes').upsert(
            homeSnapshotPayload(home, userId: userId),
            onConflict: 'id',
          );

      var syncedServerHubCount = 0;
      for (final hub in serverHubs) {
        final encryptedToken = await _encryptedHubToken(hub);
        if (hubTokenEncryptionUnavailableForTesting(
          hub: hub,
          encryptedToken: encryptedToken,
        )) {
          debugPrint(
            'AccountCloudSyncService: hub token encryption unavailable for '
            'hub=${hub.id} — encrypted_token not synced',
          );
        }
        try {
          await _upsertServerHub(
            client,
            hub,
            encryptedToken,
            clearRemoteEndpoint: clearRemoteEndpointHubIds.contains(hub.id),
          );
          syncedServerHubCount += 1;
        } catch (error) {
          if (!homeAlreadyInCloud &&
              syncedServerHubCount == 0 &&
              _isServerIdentityUniqueConflict(error)) {
            await _deleteCloudHomeIfEmpty(
              client,
              homeId: home.id,
              reason: 'server_identity_conflict',
            );
          }
          rethrow;
        }
      }

      debugPrint(
        'AccountCloudSyncService: synced home=${home.id} '
        'server_hubs=${serverHubs.length} reason=$reason',
      );
    } catch (error, stackTrace) {
      debugPrint(
        'AccountCloudSyncService: sync skipped for home=${home.id} '
        'reason=$reason error=$error',
      );
      debugPrint('$stackTrace');
    }
  }

  Future<void> deleteHub({
    required String hubId,
    required String reason,
  }) async {
    final client = _client;
    if (!canUseSignedInCloudFeatures || client == null) return;

    try {
      await client.from('hubs').delete().eq('id', hubId);
      debugPrint(
        'AccountCloudSyncService: deleted hub=$hubId reason=$reason',
      );
    } catch (error) {
      debugPrint(
        'AccountCloudSyncService: hub delete skipped for hub=$hubId '
        'reason=$reason error=$error',
      );
    }
  }

  Future<void> deleteHome({
    required String homeId,
    required String reason,
  }) async {
    final client = _client;
    if (!canUseSignedInCloudFeatures || client == null) return;

    try {
      await client.from('homes').delete().eq('id', homeId);
      debugPrint(
        'AccountCloudSyncService: deleted home=$homeId reason=$reason',
      );
    } catch (error) {
      debugPrint(
        'AccountCloudSyncService: home delete skipped for home=$homeId '
        'reason=$reason error=$error',
      );
    }
  }

  static Map<String, dynamic> homeSnapshotPayload(
    Home home, {
    String? userId,
  }) {
    return {
      'id': home.id,
      'name': home.name,
      if (userId != null) 'owner_id': userId,
      if (userId != null) 'member_ids': [userId],
      if (home.location != null) 'location': home.location!.toJson(),
      'sleep_schedule': home.sleepSchedule.toJson(),
      if (home.curveConfigJson != null) 'curve_config': home.curveConfigJson,
      if (home.timezone != null) 'timezone': home.timezone,
      'created_at': home.createdAt.toUtc().toIso8601String(),
      'updated_at': home.updatedAt.toUtc().toIso8601String(),
    };
  }

  static Map<String, dynamic> serverHubSnapshotPayload(
    Hub hub, {
    Map<String, dynamic>? encryptedToken,
    bool useLegacyEncryptedTokenStorage = false,
    bool clearRemoteEndpoint = false,
    bool includeServerInstanceId = true,
  }) {
    if (hub.type != HubType.server) {
      throw ArgumentError(
          'Only Rhythm Server hubs can be synced as server hubs.');
    }

    final serverInstanceId = _cleanServerInstanceId(hub.serverInstanceId);
    return {
      'id': hub.id,
      'home_id': hub.homeId,
      'type': hub.type.name,
      'name': hub.name,
      'endpoint': hub.endpoint.toJson(),
      'enabled': hub.enabled,
      if (includeServerInstanceId && serverInstanceId != null)
        'server_instance_id': serverInstanceId,
      if (hub.remoteEndpoint != null || clearRemoteEndpoint)
        'remote_endpoint':
            clearRemoteEndpoint ? null : hub.remoteEndpoint?.toJson(),
      'token': useLegacyEncryptedTokenStorage && encryptedToken != null
          ? jsonEncode(encryptedToken)
          : null,
      if (!useLegacyEncryptedTokenStorage && encryptedToken != null)
        'encrypted_token': encryptedToken,
      if (hub.lastConnected != null)
        'last_connected': hub.lastConnected!.toUtc().toIso8601String(),
      'created_at': hub.createdAt.toUtc().toIso8601String(),
      'updated_at': hub.updatedAt.toUtc().toIso8601String(),
    };
  }

  Future<Map<String, dynamic>?> _encryptedHubToken(Hub hub) {
    final token = hub.token?.trim();
    if (token == null || token.isEmpty) {
      return Future.value(null);
    }
    return AccountDataEncryptionService.instance.encryptStringForCurrentUser(
      token,
      purpose: hubTokenPurpose(homeId: hub.homeId, hubId: hub.id),
    );
  }

  Future<String?> _decryptHubToken(Map<String, dynamic> row) {
    final encryptedToken = row['encrypted_token'];
    final envelope = encryptedToken is Map
        ? Map<String, dynamic>.from(encryptedToken)
        : _encryptedEnvelopeFromLegacyToken(row['token']);
    if (envelope == null) return Future.value(null);

    return AccountDataEncryptionService.instance.decryptStringForCurrentUser(
      envelope,
      purpose: hubTokenPurpose(
        homeId: row['home_id'] as String,
        hubId: row['id'] as String,
      ),
    );
  }

  Future<Iterable<Map>> _loadServerHubRows(
    SupabaseClient client,
    List<String> homeIds,
  ) async {
    final attempts = <({String columns, String label})>[
      (columns: _serverHubColumns, label: 'current'),
      (
        columns: _serverHubColumnsWithoutServerInstanceId,
        label: 'without server_instance_id',
      ),
      (
        columns: _legacyServerHubColumnsWithServerInstanceId,
        label: 'legacy token with server_instance_id',
      ),
      (columns: _legacyServerHubColumns, label: 'legacy token'),
    ];

    Object? lastMissingColumnError;
    for (final attempt in attempts) {
      try {
        return await _selectServerHubRows(client, homeIds, attempt.columns);
      } catch (error) {
        if (!_isMissingColumnError(error, 'encrypted_token') &&
            !_isMissingColumnError(error, 'server_instance_id')) {
          rethrow;
        }
        lastMissingColumnError = error;
        debugPrint(
          'AccountCloudSyncService: ${attempt.label} hub columns unavailable; '
          'trying compatibility fallback',
        );
      }
    }
    throw lastMissingColumnError ??
        StateError('No compatible account hub column set available');
  }

  Future<Iterable<Map>> _selectServerHubRows(
    SupabaseClient client,
    List<String> homeIds,
    String columns,
  ) async {
    final rows = await client
        .from('hubs')
        .select(columns)
        .eq('type', HubType.server.name)
        .inFilter('home_id', homeIds)
        .order('updated_at', ascending: false);
    return rows.whereType<Map>();
  }

  Future<void> _upsertServerHub(
    SupabaseClient client,
    Hub hub,
    Map<String, dynamic>? encryptedToken, {
    required bool clearRemoteEndpoint,
  }) async {
    final attempts = <({
      bool includeServerInstanceId,
      bool useLegacyEncryptedTokenStorage,
      String label,
    })>[
      (
        includeServerInstanceId: true,
        useLegacyEncryptedTokenStorage: false,
        label: 'current',
      ),
      (
        includeServerInstanceId: false,
        useLegacyEncryptedTokenStorage: false,
        label: 'without server_instance_id',
      ),
      (
        includeServerInstanceId: true,
        useLegacyEncryptedTokenStorage: true,
        label: 'legacy token with server_instance_id',
      ),
      (
        includeServerInstanceId: false,
        useLegacyEncryptedTokenStorage: true,
        label: 'legacy token',
      ),
    ];

    Object? lastMissingColumnError;
    for (final attempt in attempts) {
      try {
        await client.from('hubs').upsert(
              serverHubSnapshotPayload(
                hub,
                encryptedToken: encryptedToken,
                clearRemoteEndpoint: clearRemoteEndpoint,
                includeServerInstanceId: attempt.includeServerInstanceId,
                useLegacyEncryptedTokenStorage:
                    attempt.useLegacyEncryptedTokenStorage,
              ),
              onConflict: 'id',
            );
        return;
      } catch (error) {
        if (!_isMissingColumnError(error, 'encrypted_token') &&
            !_isMissingColumnError(error, 'server_instance_id')) {
          rethrow;
        }
        lastMissingColumnError = error;
        debugPrint(
          'AccountCloudSyncService: ${attempt.label} hub upsert unavailable; '
          'trying compatibility fallback',
        );
      }
    }
    throw lastMissingColumnError ??
        StateError('No compatible account hub upsert payload available');
  }

  Future<bool> _accountHomeExists(SupabaseClient client, String homeId) async {
    try {
      final row = await client
          .from('homes')
          .select('id')
          .eq('id', homeId)
          .maybeSingle();
      return row != null;
    } catch (error) {
      debugPrint(
        'AccountCloudSyncService: home existence check failed for '
        'home=$homeId: $error',
      );
      return true;
    }
  }

  Future<void> _deleteCloudHomeIfEmpty(
    SupabaseClient client, {
    required String homeId,
    required String reason,
  }) async {
    try {
      final hubRows =
          await client.from('hubs').select('id').eq('home_id', homeId).limit(1);
      if (hubRows.whereType<Map>().isNotEmpty) return;

      await client.from('homes').delete().eq('id', homeId);
      debugPrint(
        'AccountCloudSyncService: deleted empty cloud home=$homeId '
        'reason=$reason',
      );
    } catch (error) {
      debugPrint(
        'AccountCloudSyncService: empty cloud home cleanup skipped for '
        'home=$homeId reason=$reason error=$error',
      );
    }
  }

  Future<
      ({
        String serverInstanceId,
        String existingHubId,
        String localHubId,
      })?> _findServerHubIdentityConflict(
    SupabaseClient client, {
    required String homeId,
    required Iterable<Hub> serverHubs,
  }) async {
    final localHubsByIdentity = <String, Hub>{};
    for (final hub in serverHubs) {
      final serverInstanceId = _cleanServerInstanceId(hub.serverInstanceId);
      if (serverInstanceId == null) continue;
      final existingLocal = localHubsByIdentity[serverInstanceId];
      if (existingLocal != null && existingLocal.id != hub.id) {
        return (
          serverInstanceId: serverInstanceId,
          existingHubId: existingLocal.id,
          localHubId: hub.id,
        );
      }
      localHubsByIdentity[serverInstanceId] = hub;
    }
    if (localHubsByIdentity.isEmpty) return null;

    try {
      final rows = await client
          .from('hubs')
          .select('id,home_id,server_instance_id')
          .eq('type', HubType.server.name)
          .inFilter(
            'server_instance_id',
            localHubsByIdentity.keys.toList(growable: false),
          )
          .limit(50);

      for (final row in rows.whereType<Map>()) {
        final existingHubId = row['id']?.toString();
        final existingHomeId = row['home_id']?.toString();
        final serverInstanceId =
            _cleanServerInstanceId(row['server_instance_id']?.toString());
        if (existingHubId == null ||
            existingHomeId == null ||
            serverInstanceId == null) {
          continue;
        }
        final localHub = localHubsByIdentity[serverInstanceId];
        if (localHub == null) continue;
        if (existingHubId != localHub.id || existingHomeId != homeId) {
          return (
            serverInstanceId: serverInstanceId,
            existingHubId: existingHubId,
            localHubId: localHub.id,
          );
        }
      }
    } catch (error) {
      if (_isMissingColumnError(error, 'server_instance_id')) {
        return null;
      }
      rethrow;
    }

    return null;
  }

  Map<String, dynamic>? _encryptedEnvelopeFromLegacyToken(Object? token) {
    if (token is! String || token.trim().isEmpty) return null;
    try {
      final decoded = jsonDecode(token);
      if (decoded is! Map) return null;
      final envelope = Map<String, dynamic>.from(decoded);
      if (envelope['version'] != AccountDataEncryptionService.envelopeVersion) {
        return null;
      }
      return envelope;
    } catch (_) {
      return null;
    }
  }

  bool _isMissingColumnError(Object error, String column) {
    final message = error.toString();
    return message.contains(column) &&
        (message.contains('schema cache') ||
            message.contains('column') ||
            message.contains('PGRST204') ||
            message.contains('42703'));
  }

  bool _isServerIdentityUniqueConflict(Object error) {
    return isServerIdentityUniqueConflictForTesting(error);
  }

  SupabaseClient? get _client {
    if (!BackendProvider.isInitialized) return null;
    final auth = BackendProvider.instance.auth;
    if (auth is SupabaseAuthBackend) return auth.client;
    return null;
  }
}

@visibleForTesting
bool isServerIdentityUniqueConflictForTesting(Object error) {
  final message = error.toString();
  final isUniqueViolation =
      error is PostgrestException ? error.code == '23505' : true;
  if (!isUniqueViolation) return false;

  return message.contains('hubs_server_instance_id_unique_idx') ||
      message.contains('hub_remote_access_server_instance_id_unique_idx') ||
      (message.contains('duplicate key') &&
          message.contains('server_instance_id'));
}

/// True when a hub has a local owner token but encryption produced no
/// envelope, meaning `encrypted_token` will not be synced for it.
@visibleForTesting
bool hubTokenEncryptionUnavailableForTesting({
  required Hub hub,
  required Map<String, dynamic>? encryptedToken,
}) {
  final token = hub.token?.trim();
  return encryptedToken == null && token != null && token.isNotEmpty;
}

@visibleForTesting
bool shouldSyncHomeAndServerHubsForTesting({
  required Home? home,
  required Iterable<Hub> serverHubs,
}) {
  return home != null && serverHubs.any((hub) => hub.type == HubType.server);
}

@visibleForTesting
bool accountHomeBelongsToUserForTesting({
  required Home home,
  required String? userId,
}) {
  final cleanUserId = userId?.trim();
  if (cleanUserId == null || cleanUserId.isEmpty) return false;
  return home.ownerId == cleanUserId || home.memberIds.contains(cleanUserId);
}

bool accountHomeCanSyncForUserForTesting({
  required Home home,
  required String? userId,
}) {
  if (accountHomeBelongsToUserForTesting(home: home, userId: userId)) {
    return true;
  }
  final ownerId = home.ownerId.trim();
  return ownerId.isEmpty ||
      ownerId == 'anonymous-user' ||
      ownerId == 'web-local';
}

@visibleForTesting
List<AccountHomeServerHubs> accountHomeServerHubsFromRowsForTesting({
  required Iterable<Home> homes,
  required Iterable<Hub> serverHubs,
}) {
  final hubsByHomeId = <String, List<Hub>>{};
  for (final hub in serverHubs) {
    if (hub.type != HubType.server) continue;
    final hubs = hubsByHomeId.putIfAbsent(hub.homeId, () => []);
    final matchIndex = hubs.indexWhere(
      (existing) => _accountServerHubsRepresentSameBox(existing, hub),
    );
    if (matchIndex == -1) {
      hubs.add(hub);
    } else {
      hubs[matchIndex] = _mergeAccountServerHubRows(hubs[matchIndex], hub);
    }
  }

  return [
    for (final home in homes)
      AccountHomeServerHubs(
        home: home,
        serverHubs: List.unmodifiable(hubsByHomeId[home.id] ?? const []),
      ),
  ];
}

bool _accountServerHubsRepresentSameBox(Hub left, Hub right) {
  if (_sameNonEmptyServerInstanceId(left, right)) return true;
  if (_differentNonEmptyServerInstanceIds(left, right)) return false;

  if (left.id == right.id) return true;
  if (_sameEndpoint(left.endpoint, right.endpoint)) return true;
  final leftRemote = left.remoteEndpoint;
  final rightRemote = right.remoteEndpoint;
  if (leftRemote != null &&
      rightRemote != null &&
      _sameEndpoint(leftRemote, rightRemote)) {
    return true;
  }
  if (_sameNonEmptyToken(left.token, right.token)) return true;

  return false;
}

Hub _mergeAccountServerHubRows(Hub base, Hub incoming) {
  final baseToken = base.token?.trim();
  final incomingToken = incoming.token?.trim();
  final baseHasRemote = base.remoteEndpoint != null;
  final incomingHasRemote = incoming.remoteEndpoint != null;
  final winner = incomingHasRemote && !baseHasRemote ? incoming : base;
  final fallback = identical(winner, base) ? incoming : base;

  return winner.copyWith(
    token: baseToken != null && baseToken.isNotEmpty
        ? base.token
        : incomingToken != null && incomingToken.isNotEmpty
            ? incoming.token
            : winner.token,
    lastConnected:
        _latestNullableDate(base.lastConnected, incoming.lastConnected),
    updatedAt: _latestDate(base.updatedAt, incoming.updatedAt),
    enabled: base.enabled || incoming.enabled,
    remoteEndpoint: winner.remoteEndpoint ?? fallback.remoteEndpoint,
    serverInstanceId: winner.serverInstanceId ?? fallback.serverInstanceId,
  );
}

bool _sameEndpoint(HubEndpoint left, HubEndpoint right) {
  return left.host == right.host &&
      left.port == right.port &&
      left.useSsl == right.useSsl;
}

bool _sameNonEmptyToken(String? left, String? right) {
  final cleanLeft = left?.trim();
  final cleanRight = right?.trim();
  return cleanLeft != null &&
      cleanLeft.isNotEmpty &&
      cleanRight != null &&
      cleanRight.isNotEmpty &&
      cleanLeft == cleanRight;
}

bool _sameNonEmptyServerInstanceId(Hub left, Hub right) {
  final leftId = _cleanServerInstanceId(left.serverInstanceId);
  final rightId = _cleanServerInstanceId(right.serverInstanceId);
  return leftId != null && rightId != null && leftId == rightId;
}

bool _differentNonEmptyServerInstanceIds(Hub left, Hub right) {
  final leftId = _cleanServerInstanceId(left.serverInstanceId);
  final rightId = _cleanServerInstanceId(right.serverInstanceId);
  return leftId != null && rightId != null && leftId != rightId;
}

String? _cleanServerInstanceId(String? value) {
  final clean = value?.trim().toLowerCase();
  if (clean == null || clean.isEmpty) return null;
  return clean;
}

DateTime _latestDate(DateTime left, DateTime right) {
  return left.isAfter(right) ? left : right;
}

DateTime? _latestNullableDate(DateTime? left, DateTime? right) {
  if (left == null) return right;
  if (right == null) return left;
  return _latestDate(left, right);
}

Map<String, dynamic> _stringKeyMap(Map<dynamic, dynamic> row) {
  return row.map((key, value) => MapEntry(key.toString(), value));
}
