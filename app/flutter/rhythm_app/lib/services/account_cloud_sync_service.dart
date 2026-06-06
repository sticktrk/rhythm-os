import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:supabase_flutter/supabase_flutter.dart';

import '../backend/backend.dart';
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

  bool get canUseSignedInCloudFeatures {
    final auth = AuthService();
    return _client != null &&
        auth.isSignedIn &&
        !auth.isAnonymous &&
        auth.currentUserId != null;
  }

  Future<List<AccountHomeServerHubs>> loadHomesAndServerHubs() async {
    final client = _client;
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
        .toList(growable: false);
    if (homes.isEmpty) return const [];

    final homeIds = homes.map((home) => home.id).toList(growable: false);
    final hubRows = await client
        .from('hubs')
        .select(
          'id,home_id,type,name,endpoint,enabled,remote_endpoint,last_connected,created_at,updated_at',
        )
        .eq('type', HubType.server.name)
        .inFilter('home_id', homeIds)
        .order('updated_at', ascending: false);
    final hubs = hubRows
        .whereType<Map>()
        .map((row) => Hub.fromSupabase(_stringKeyMap(row)))
        .toList(growable: false);

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

    try {
      await client.from('homes').upsert(
            homeSnapshotPayload(home, userId: userId),
            onConflict: 'id',
          );

      for (final hub in serverHubs) {
        await client.from('hubs').upsert(
              serverHubSnapshotPayload(hub),
              onConflict: 'id',
            );
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

  static Map<String, dynamic> serverHubSnapshotPayload(Hub hub) {
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
      'remote_endpoint': hub.remoteEndpoint?.toJson(),
      if (hub.lastConnected != null)
        'last_connected': hub.lastConnected!.toUtc().toIso8601String(),
      'created_at': hub.createdAt.toUtc().toIso8601String(),
      'updated_at': hub.updatedAt.toUtc().toIso8601String(),
    };
  }

  SupabaseClient? get _client {
    if (!BackendProvider.isInitialized) return null;
    final auth = BackendProvider.instance.auth;
    if (auth is SupabaseAuthBackend) return auth.client;
    return null;
  }
}

@visibleForTesting
List<AccountHomeServerHubs> accountHomeServerHubsFromRowsForTesting({
  required Iterable<Home> homes,
  required Iterable<Hub> serverHubs,
}) {
  final hubsByHomeId = <String, List<Hub>>{};
  for (final hub in serverHubs) {
    if (hub.type != HubType.server) continue;
    hubsByHomeId.putIfAbsent(hub.homeId, () => []).add(hub);
  }

  return [
    for (final home in homes)
      AccountHomeServerHubs(
        home: home,
        serverHubs: List.unmodifiable(hubsByHomeId[home.id] ?? const []),
      ),
  ];
}

Map<String, dynamic> _stringKeyMap(Map<dynamic, dynamic> row) {
  return row.map((key, value) => MapEntry(key.toString(), value));
}
