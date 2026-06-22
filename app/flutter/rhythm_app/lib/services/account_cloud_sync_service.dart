import 'dart:async';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:supabase_flutter/supabase_flutter.dart';

import '../backend/backend.dart';
import 'account_data_encryption_service.dart';
import 'auth_service.dart';
import 'employee_mode_service.dart';

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
      'id,home_id,type,name,endpoint,enabled,token,encrypted_token,remote_endpoint,last_connected,created_at,updated_at';
  static const _legacyServerHubColumns =
      'id,home_id,type,name,endpoint,enabled,token,remote_endpoint,last_connected,created_at,updated_at';

  bool get canUseSignedInCloudFeatures {
    final auth = AuthService();
    return _client != null &&
        !EmployeeModeService.instance.isActive &&
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
  }) async {
    if (EmployeeModeService.instance.isActive) {
      debugPrint(
        'AccountCloudSyncService: sync skipped in employee mode reason=$reason',
      );
      return;
    }

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
      await client.from('homes').upsert(
            homeSnapshotPayload(home, userId: userId),
            onConflict: 'id',
          );

      for (final hub in serverHubs) {
        final encryptedToken = await _encryptedHubToken(hub);
        await _upsertServerHub(client, hub, encryptedToken);
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

  Future<DateTime?> setSupportAccessConsent({
    required String homeId,
    required bool accepted,
  }) async {
    final client = _client;
    final auth = AuthService();
    final userId = auth.currentUserId;
    if (client == null ||
        EmployeeModeService.instance.isActive ||
        !auth.isSignedIn ||
        auth.isAnonymous ||
        userId == null) {
      throw StateError('Support access consent requires a signed-in owner.');
    }

    final timestamp = accepted ? DateTime.now().toUtc() : null;
    final row = await client
        .from('homes')
        .update({
          'support_access_consent_at': timestamp?.toIso8601String(),
        })
        .eq('id', homeId)
        .eq('owner_id', userId)
        .select('id')
        .maybeSingle();
    if (row == null) {
      throw StateError('Support access consent can only be set by the owner.');
    }
    return timestamp;
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
  }) {
    if (hub.type != HubType.server) {
      throw ArgumentError(
          'Only Rhythm Server hubs can be synced as server hubs.');
    }

    return {
      'id': hub.id,
      'home_id': hub.homeId,
      'type': hub.type.name,
      'name': hub.name,
      'endpoint': hub.endpoint.toJson(),
      'enabled': hub.enabled,
      if (hub.remoteEndpoint != null || clearRemoteEndpoint)
        'remote_endpoint': hub.remoteEndpoint?.toJson(),
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
    try {
      return await _selectServerHubRows(client, homeIds, _serverHubColumns);
    } catch (error) {
      if (!_isMissingColumnError(error, 'encrypted_token')) rethrow;
      debugPrint(
        'AccountCloudSyncService: encrypted_token column unavailable; '
        'loading legacy encrypted token storage',
      );
      return _selectServerHubRows(client, homeIds, _legacyServerHubColumns);
    }
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
    Map<String, dynamic>? encryptedToken,
  ) async {
    try {
      await client.from('hubs').upsert(
            serverHubSnapshotPayload(
              hub,
              encryptedToken: encryptedToken,
            ),
            onConflict: 'id',
          );
    } catch (error) {
      if (!_isMissingColumnError(error, 'encrypted_token')) rethrow;
      debugPrint(
        'AccountCloudSyncService: encrypted_token column unavailable; '
        'saving encrypted token envelope in legacy token column',
      );
      await client.from('hubs').upsert(
            serverHubSnapshotPayload(
              hub,
              encryptedToken: encryptedToken,
              useLegacyEncryptedTokenStorage: true,
            ),
            onConflict: 'id',
          );
    }
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

  SupabaseClient? get _client {
    if (!BackendProvider.isInitialized) return null;
    final auth = BackendProvider.instance.auth;
    if (auth is SupabaseAuthBackend) return auth.client;
    return null;
  }
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
  if (left.id == right.id) return true;
  if (_sameEndpoint(left.endpoint, right.endpoint)) return true;
  final leftRemote = left.remoteEndpoint;
  final rightRemote = right.remoteEndpoint;
  if (leftRemote != null &&
      rightRemote != null &&
      _sameEndpoint(leftRemote, rightRemote)) {
    return true;
  }

  return left.type == HubType.server &&
      right.type == HubType.server &&
      _normalizedName(left.name) == _normalizedName(right.name) &&
      (leftRemote != null || rightRemote != null);
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
  );
}

bool _sameEndpoint(HubEndpoint left, HubEndpoint right) {
  return left.host == right.host &&
      left.port == right.port &&
      left.useSsl == right.useSsl;
}

String _normalizedName(String name) => name.trim().toLowerCase();

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
