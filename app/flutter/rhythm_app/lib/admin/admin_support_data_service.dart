import 'package:flutter/foundation.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:supabase_flutter/supabase_flutter.dart';

import '../backend/backend.dart';
import '../services/account_cloud_sync_service.dart';
import '../services/auth_service.dart';

class AdminSupportSnapshot {
  const AdminSupportSnapshot({
    required this.customers,
    required this.staffStatus,
  });

  static const empty = AdminSupportSnapshot(
    customers: [],
    staffStatus: SupportStaffStatus.unknown(),
  );

  final List<SupportCustomerHomes> customers;
  final SupportStaffStatus staffStatus;
}

class SupportCustomerHomes {
  const SupportCustomerHomes({
    required this.customerLabel,
    this.customerEmail,
    this.customerName,
    required this.ownerId,
    required this.homes,
  });

  final String customerLabel;
  final String? customerEmail;
  final String? customerName;
  final String ownerId;
  final List<AccountHomeServerHubs> homes;

  String? get secondaryLabel {
    final email = customerEmail?.trim();
    final name = customerName?.trim();
    if (email == null || email.isEmpty) return null;
    if (name == null || name.isEmpty) return null;
    if (email.toLowerCase() == name.toLowerCase()) return null;
    return email;
  }
}

class SupportStaffStatus {
  const SupportStaffStatus({
    required this.role,
    required this.enabled,
  });

  const SupportStaffStatus.none()
      : role = null,
        enabled = false;

  const SupportStaffStatus.unknown()
      : role = 'unknown',
        enabled = false;

  final String? role;
  final bool enabled;

  bool get isActive {
    final cleanRole = role?.trim();
    return enabled && cleanRole != null && cleanRole.isNotEmpty;
  }

  bool get isUnknown => role == 'unknown' && !enabled;

  String get label {
    final cleanRole = role?.trim();
    if (cleanRole == null || cleanRole.isEmpty) return 'none';
    return enabled ? cleanRole : '$cleanRole-disabled';
  }
}

class AdminSupportDataService {
  AdminSupportDataService._();

  static final AdminSupportDataService instance = AdminSupportDataService._();

  Future<AdminSupportSnapshot> loadCustomersAndServerHubs() async {
    final auth = AuthService();
    final client = _client;
    final currentUserId = auth.currentUserId;
    if (client == null ||
        !auth.isSignedIn ||
        auth.isAnonymous ||
        currentUserId == null) {
      return AdminSupportSnapshot.empty;
    }
    final staffStatus = await _loadStaffStatus(client, currentUserId);

    final homeRows = await client
        .from('homes')
        .select(
          'id,name,owner_id,member_ids,location,sleep_schedule,curve_config,timezone,created_at,updated_at',
        )
        .order('updated_at', ascending: false);
    final homes = homeRows
        .whereType<Map>()
        .map((row) => Home.fromSupabase(_modelMap(row)))
        .toList(growable: false);
    debugPrint(
      'AdminSupportDataService: staff=${staffStatus.label} '
      'visible_homes=${homes.length}',
    );
    if (homes.isEmpty) {
      return AdminSupportSnapshot(
        customers: const [],
        staffStatus: staffStatus,
      );
    }

    final homeIds = homes.map((home) => home.id).toSet();
    final supportHubs = await _loadSupportHubs(client, homeIds);
    final visibleHubsById = <String, Hub>{
      for (final hub in supportHubs) hub.id: hub,
    };

    // The staff support view intentionally hides owner tokens. For homes owned
    // by the signed-in account, keep the existing account loader's decrypted
    // token so live device counts and local support actions still work.
    final ownSnapshots =
        await AccountCloudSyncService.instance.loadHomesAndServerHubs();
    for (final snapshot in ownSnapshots) {
      for (final hub in snapshot.serverHubs) {
        if (!homeIds.contains(hub.homeId)) continue;
        visibleHubsById[hub.id] = _mergePrivilegedHub(
          supportHub: visibleHubsById[hub.id],
          accountHub: hub,
        );
      }
    }

    final identities = await _loadCustomerIdentities(
      client,
      homes.map((home) => home.ownerId).toSet(),
    );

    return AdminSupportSnapshot(
      staffStatus: staffStatus,
      customers: _groupByCustomer(
        homes: homes,
        hubs: visibleHubsById.values,
        currentUserId: currentUserId,
        currentUserEmail: auth.currentUser?.email,
        identities: identities,
      ),
    );
  }

  Future<List<Hub>> _loadSupportHubs(
    SupabaseClient client,
    Set<String> homeIds,
  ) async {
    if (homeIds.isEmpty) return const [];
    try {
      final rows = await client
          .from('rhythm_support_hubs')
          .select(
            'id,home_id,type,name,endpoint,enabled,remote_endpoint,server_instance_id,last_connected,created_at,updated_at',
          )
          .eq('type', HubType.server.name)
          .inFilter('home_id', homeIds.toList(growable: false))
          .order('updated_at', ascending: false);
      return rows
          .whereType<Map>()
          .map((row) => Hub.fromSupabase(_modelMap(row)))
          .toList(growable: false);
    } catch (error) {
      debugPrint('AdminSupportDataService: support hubs unavailable: $error');
      return const [];
    }
  }

  Future<SupportStaffStatus> _loadStaffStatus(
    SupabaseClient client,
    String currentUserId,
  ) async {
    try {
      final rows = await client
          .from('rhythm_staff')
          .select('role,enabled')
          .eq('user_id', currentUserId)
          .limit(1);
      final first = rows.whereType<Map>().firstOrNull;
      if (first == null) return const SupportStaffStatus.none();
      return SupportStaffStatus(
        role: first['role'] as String?,
        enabled: first['enabled'] == true,
      );
    } catch (error) {
      debugPrint('AdminSupportDataService: staff status unavailable: $error');
      return const SupportStaffStatus.unknown();
    }
  }

  Future<Map<String, _SupportCustomerIdentity>> _loadCustomerIdentities(
    SupabaseClient client,
    Set<String> ownerIds,
  ) async {
    if (ownerIds.isEmpty) return const {};
    try {
      final rows = await client
          .from('rhythm_support_customers')
          .select('user_id,email,name')
          .inFilter('user_id', ownerIds.toList(growable: false));
      return {
        for (final row in rows.whereType<Map>())
          if (row['user_id'] is String)
            row['user_id'] as String: _SupportCustomerIdentity(
              email: row['email'] as String?,
              name: row['name'] as String?,
            ),
      };
    } catch (error) {
      debugPrint(
        'AdminSupportDataService: support customer identity unavailable: '
        '$error',
      );
      return const {};
    }
  }

  List<SupportCustomerHomes> _groupByCustomer({
    required List<Home> homes,
    required Iterable<Hub> hubs,
    required String currentUserId,
    required String? currentUserEmail,
    required Map<String, _SupportCustomerIdentity> identities,
  }) {
    final hubsByHomeId = <String, List<Hub>>{};
    for (final hub in hubs) {
      if (hub.type != HubType.server) continue;
      hubsByHomeId.putIfAbsent(hub.homeId, () => []).add(hub);
    }

    final grouped = <String, List<AccountHomeServerHubs>>{};
    for (final home in homes) {
      grouped.putIfAbsent(home.ownerId, () => []).add(
            AccountHomeServerHubs(
              home: home,
              serverHubs: List.unmodifiable(hubsByHomeId[home.id] ?? const []),
            ),
          );
    }

    return [
      for (final entry in grouped.entries)
        () {
          final identity = identities[entry.key];
          final label = _customerLabel(
            ownerId: entry.key,
            currentUserId: currentUserId,
            currentUserEmail: currentUserEmail,
            identity: identity,
          );
          return SupportCustomerHomes(
            ownerId: entry.key,
            customerLabel: label,
            customerEmail: identity?.email ??
                (entry.key == currentUserId ? currentUserEmail : null),
            customerName: identity?.name,
            homes: List.unmodifiable(entry.value),
          );
        }(),
    ];
  }

  Hub _mergePrivilegedHub({
    required Hub? supportHub,
    required Hub accountHub,
  }) {
    if (supportHub == null) return accountHub;
    return accountHub.copyWith(
      remoteEndpoint: accountHub.remoteEndpoint ?? supportHub.remoteEndpoint,
      serverInstanceId:
          accountHub.serverInstanceId ?? supportHub.serverInstanceId,
      lastConnected: _latestNullableDate(
        accountHub.lastConnected,
        supportHub.lastConnected,
      ),
      updatedAt: _latestDate(accountHub.updatedAt, supportHub.updatedAt),
      enabled: accountHub.enabled || supportHub.enabled,
    );
  }

  DateTime? _latestNullableDate(DateTime? left, DateTime? right) {
    if (left == null) return right;
    if (right == null) return left;
    return _latestDate(left, right);
  }

  DateTime _latestDate(DateTime left, DateTime right) {
    return left.isAfter(right) ? left : right;
  }

  String _customerLabel({
    required String ownerId,
    required String currentUserId,
    required String? currentUserEmail,
    required _SupportCustomerIdentity? identity,
  }) {
    final identityName = identity?.name?.trim();
    if (identityName != null && identityName.isNotEmpty) return identityName;
    final identityEmail = identity?.email?.trim();
    if (identityEmail != null && identityEmail.isNotEmpty) {
      return identityEmail;
    }
    if (ownerId == currentUserId &&
        currentUserEmail != null &&
        currentUserEmail.trim().isNotEmpty) {
      return currentUserEmail;
    }
    return 'Customer ${_shortId(ownerId)}';
  }

  String _shortId(String value) {
    final clean = value.trim();
    if (clean.length <= 8) return clean.isEmpty ? 'unknown' : clean;
    return clean.substring(0, 8);
  }

  Map<String, dynamic> _modelMap(Map<dynamic, dynamic> row) {
    return row.map((key, value) {
      if (value is Map) {
        return MapEntry(key.toString(), Map<String, dynamic>.from(value));
      }
      if (value is List) {
        return MapEntry(key.toString(), List<dynamic>.from(value));
      }
      return MapEntry(key.toString(), value);
    });
  }

  SupabaseClient? get _client {
    if (!BackendProvider.isInitialized) return null;
    final auth = BackendProvider.instance.auth;
    if (auth is SupabaseAuthBackend) return auth.client;
    return null;
  }
}

class _SupportCustomerIdentity {
  const _SupportCustomerIdentity({
    this.email,
    this.name,
  });

  final String? email;
  final String? name;
}
