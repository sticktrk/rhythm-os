import 'models.dart';
import 'supabase_rest_client.dart';

class SupportService {
  const SupportService({required SupabaseRestClient supabase})
      : _supabase = supabase;

  final SupabaseRestClient _supabase;

  Future<SupportSnapshotDto> loadSnapshot(AdminSession session) async {
    final hubRows = await _loadActiveServerHubs(session);
    final hubs = hubRows
        .map(SupportHubDto.fromSupabase)
        .where((hub) => hub.id.isNotEmpty && hub.homeId.isNotEmpty)
        .toList(growable: false);
    if (hubs.isEmpty) {
      return SupportSnapshotDto(
        staffStatus: session.staff,
        customers: const [],
      );
    }

    final homeIds = hubs.map((hub) => hub.homeId).toSet();
    final homeRows = await _supabase.select(
      table: 'homes',
      select:
          'id,name,owner_id,member_ids,location,sleep_schedule,curve_config,timezone,created_at,updated_at',
      accessToken: session.accessToken,
      filters: {
        'id': postgrestInFilter(homeIds),
      },
      order: 'updated_at.desc',
    );
    final homes = homeRows
        .map(SupportHomeDto.fromSupabase)
        .where((home) => home.id.isNotEmpty)
        .toList(growable: false);
    if (homes.isEmpty) {
      return SupportSnapshotDto(
        staffStatus: session.staff,
        customers: const [],
      );
    }

    final hubsByHomeId = <String, List<SupportHubDto>>{};
    for (final hub in hubs) {
      hubsByHomeId.putIfAbsent(hub.homeId, () => []).add(hub);
    }

    final homesByOwner = <String, List<SupportHomeDto>>{};
    for (final home in homes) {
      homesByOwner.putIfAbsent(home.ownerId, () => []).add(home);
    }

    final customers = <SupportCustomerDto>[];
    for (final entry in homesByOwner.entries) {
      final ownerId = entry.key;
      customers.add(
        SupportCustomerDto(
          ownerId: ownerId,
          customerLabel: _customerLabel(ownerId),
          homes: entry.value
              .map(
                (home) => SupportCustomerHomeDto(
                  home: home,
                  hubs: hubsByHomeId[home.id] ?? const [],
                ),
              )
              .toList(growable: false),
        ),
      );
    }
    customers.sort(
      (left, right) => left.customerLabel.toLowerCase().compareTo(
            right.customerLabel.toLowerCase(),
          ),
    );

    return SupportSnapshotDto(
      staffStatus: session.staff,
      customers: customers,
    );
  }

  Future<List<Map<String, dynamic>>> _loadActiveServerHubs(
    AdminSession session,
  ) {
    final serviceRole = _supabase.canUseServiceRole;
    return _supabase.select(
      table: serviceRole ? 'hubs' : 'rhythm_support_hubs',
      select: serviceRole
          ? 'id,home_id,type,name,endpoint,enabled,token,encrypted_token,remote_endpoint,server_instance_id,last_connected,created_at,updated_at'
          : 'id,home_id,type,name,endpoint,enabled,remote_endpoint,server_instance_id,last_connected,created_at,updated_at,has_legacy_token,has_encrypted_token',
      accessToken: session.accessToken,
      serviceRole: serviceRole,
      filters: {
        'type': 'eq.server',
        'enabled': 'eq.true',
      },
      order: 'updated_at.desc',
    );
  }

  String _customerLabel(String ownerId) {
    return ownerId.length <= 8 ? ownerId : '${ownerId.substring(0, 8)}...';
  }
}

String postgrestInFilter(Iterable<String> values) {
  final literals = values
      .where((value) => value.trim().isNotEmpty)
      .map((value) => '"${value.replaceAll('"', r'\"')}"')
      .join(',');
  return 'in.($literals)';
}
