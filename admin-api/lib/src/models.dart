class AdminUser {
  const AdminUser({
    required this.id,
    required this.email,
    this.createdAt,
  });

  final String id;
  final String? email;
  final DateTime? createdAt;

  factory AdminUser.fromJson(Map<String, dynamic> json) {
    return AdminUser(
      id: json['id'] as String? ?? '',
      email: json['email'] as String?,
      createdAt: parseDateTime(json['created_at']),
    );
  }

  Map<String, dynamic> toJson() => {
        'id': id,
        if (email != null) 'email': email,
        if (createdAt != null) 'createdAt': createdAt!.toIso8601String(),
      };
}

class StaffStatus {
  const StaffStatus({
    required this.role,
    required this.enabled,
  });

  const StaffStatus.none()
      : role = null,
        enabled = false;

  final String? role;
  final bool enabled;

  bool get isActive => enabled && role != null && role!.trim().isNotEmpty;
  bool get isAdmin => isActive && role == 'admin';

  Map<String, dynamic> toJson() => {
        'role': role,
        'enabled': enabled,
        'isActive': isActive,
        'isAdmin': isAdmin,
      };
}

class AdminSession {
  const AdminSession({
    required this.accessToken,
    required this.user,
    required this.staff,
  });

  final String accessToken;
  final AdminUser user;
  final StaffStatus staff;
}

class HubEndpointDto {
  const HubEndpointDto({
    required this.host,
    required this.port,
    required this.useSsl,
  });

  final String host;
  final int port;
  final bool useSsl;

  String get baseUrl => '${useSsl ? 'https' : 'http'}://$host:$port';

  factory HubEndpointDto.fromJson(Map<String, dynamic> json) {
    return HubEndpointDto(
      host: json['host'] as String? ?? '',
      port: (json['port'] as num?)?.toInt() ?? 80,
      useSsl: json['useSsl'] as bool? ?? json['use_ssl'] as bool? ?? false,
    );
  }

  Map<String, dynamic> toJson() => {
        'host': host,
        'port': port,
        'useSsl': useSsl,
        'baseUrl': baseUrl,
      };
}

class SupportHomeDto {
  const SupportHomeDto({
    required this.id,
    required this.name,
    required this.ownerId,
    required this.memberIds,
    this.locationCity,
    this.timezone,
    this.createdAt,
    this.updatedAt,
  });

  final String id;
  final String name;
  final String ownerId;
  final List<String> memberIds;
  final String? locationCity;
  final String? timezone;
  final DateTime? createdAt;
  final DateTime? updatedAt;

  factory SupportHomeDto.fromSupabase(Map<String, dynamic> row) {
    final location = asStringMap(row['location']);
    return SupportHomeDto(
      id: row['id'] as String? ?? '',
      name: row['name'] as String? ?? 'Unnamed home',
      ownerId: row['owner_id'] as String? ?? '',
      memberIds: (row['member_ids'] as List<dynamic>? ?? const [])
          .map((value) => value.toString())
          .toList(growable: false),
      locationCity: location?['cityName'] as String?,
      timezone: row['timezone'] as String?,
      createdAt: parseDateTime(row['created_at']),
      updatedAt: parseDateTime(row['updated_at']),
    );
  }

  Map<String, dynamic> toJson() => {
        'id': id,
        'name': name,
        'ownerId': ownerId,
        'memberIds': memberIds,
        if (locationCity != null) 'locationCity': locationCity,
        if (timezone != null) 'timezone': timezone,
        if (createdAt != null) 'createdAt': createdAt!.toIso8601String(),
        if (updatedAt != null) 'updatedAt': updatedAt!.toIso8601String(),
      };
}

class SupportHubDto {
  const SupportHubDto({
    required this.id,
    required this.homeId,
    required this.type,
    required this.name,
    required this.endpoint,
    required this.enabled,
    required this.hasLegacyToken,
    required this.hasEncryptedToken,
    this.remoteEndpoint,
    this.serverInstanceId,
    this.lastConnected,
    this.createdAt,
    this.updatedAt,
  });

  final String id;
  final String homeId;
  final String type;
  final String name;
  final HubEndpointDto endpoint;
  final HubEndpointDto? remoteEndpoint;
  final bool enabled;
  final bool hasLegacyToken;
  final bool hasEncryptedToken;
  final String? serverInstanceId;
  final DateTime? lastConnected;
  final DateTime? createdAt;
  final DateTime? updatedAt;

  factory SupportHubDto.fromSupabase(Map<String, dynamic> row) {
    final endpoint = asStringMap(row['endpoint']) ?? const <String, dynamic>{};
    final remoteEndpoint = asStringMap(row['remote_endpoint']);
    return SupportHubDto(
      id: row['id'] as String? ?? '',
      homeId: row['home_id'] as String? ?? '',
      type: row['type'] as String? ?? '',
      name: row['name'] as String? ?? 'Unnamed Light Box',
      endpoint: HubEndpointDto.fromJson(endpoint),
      remoteEndpoint: remoteEndpoint == null
          ? null
          : HubEndpointDto.fromJson(remoteEndpoint),
      enabled: row['enabled'] as bool? ?? true,
      hasLegacyToken:
          row['has_legacy_token'] as bool? ?? _hasUsableString(row['token']),
      hasEncryptedToken:
          row['has_encrypted_token'] as bool? ?? row['encrypted_token'] != null,
      serverInstanceId: cleanString(row['server_instance_id']),
      lastConnected: parseDateTime(row['last_connected']),
      createdAt: parseDateTime(row['created_at']),
      updatedAt: parseDateTime(row['updated_at']),
    );
  }

  Map<String, dynamic> toJson() => {
        'id': id,
        'homeId': homeId,
        'type': type,
        'name': name,
        'endpoint': endpoint.toJson(),
        if (remoteEndpoint != null) 'remoteEndpoint': remoteEndpoint!.toJson(),
        'enabled': enabled,
        'hasLegacyToken': hasLegacyToken,
        'hasEncryptedToken': hasEncryptedToken,
        if (serverInstanceId != null) 'serverInstanceId': serverInstanceId,
        if (lastConnected != null)
          'lastConnected': lastConnected!.toIso8601String(),
        if (createdAt != null) 'createdAt': createdAt!.toIso8601String(),
        if (updatedAt != null) 'updatedAt': updatedAt!.toIso8601String(),
      };
}

class SupportCustomerIdentity {
  const SupportCustomerIdentity({
    required this.userId,
    this.email,
    this.name,
  });

  final String userId;
  final String? email;
  final String? name;

  factory SupportCustomerIdentity.fromSupabase(Map<String, dynamic> row) {
    return SupportCustomerIdentity(
      userId: row['user_id'] as String? ?? '',
      email: cleanString(row['email']),
      name: cleanString(row['name']),
    );
  }
}

class SupportCustomerHomeDto {
  const SupportCustomerHomeDto({
    required this.home,
    required this.hubs,
  });

  final SupportHomeDto home;
  final List<SupportHubDto> hubs;

  Map<String, dynamic> toJson() => {
        'home': home.toJson(),
        'hubs': hubs.map((hub) => hub.toJson()).toList(growable: false),
      };
}

class SupportCustomerDto {
  const SupportCustomerDto({
    required this.ownerId,
    required this.customerLabel,
    required this.homes,
    this.customerEmail,
    this.customerName,
  });

  final String ownerId;
  final String customerLabel;
  final String? customerEmail;
  final String? customerName;
  final List<SupportCustomerHomeDto> homes;

  String? get secondaryLabel {
    final email = customerEmail?.trim();
    final name = customerName?.trim();
    if (email == null || email.isEmpty) return null;
    if (name == null || name.isEmpty) return null;
    if (email.toLowerCase() == name.toLowerCase()) return null;
    return email;
  }

  Map<String, dynamic> toJson() => {
        'ownerId': ownerId,
        'customerLabel': customerLabel,
        if (customerEmail != null) 'customerEmail': customerEmail,
        if (customerName != null) 'customerName': customerName,
        if (secondaryLabel != null) 'secondaryLabel': secondaryLabel,
        'homes': homes.map((home) => home.toJson()).toList(growable: false),
      };
}

class SupportSnapshotDto {
  const SupportSnapshotDto({
    required this.staffStatus,
    required this.customers,
  });

  final StaffStatus staffStatus;
  final List<SupportCustomerDto> customers;

  Map<String, dynamic> toJson() {
    final homes = customers.fold<int>(
      0,
      (total, customer) => total + customer.homes.length,
    );
    final hubs = customers.fold<int>(
      0,
      (total, customer) =>
          total +
          customer.homes.fold<int>(
            0,
            (homeTotal, home) => homeTotal + home.hubs.length,
          ),
    );
    return {
      'staffStatus': staffStatus.toJson(),
      'totals': {
        'customers': customers.length,
        'homes': homes,
        'hubs': hubs,
      },
      'customers': customers
          .map((customer) => customer.toJson())
          .toList(growable: false),
    };
  }
}

class LightBoxInventoryDto {
  const LightBoxInventoryDto({
    required this.lights,
    required this.buttons,
    required this.motionSensors,
    required this.otherDevices,
  });

  final int lights;
  final int buttons;
  final int motionSensors;
  final int otherDevices;

  int get total => lights + buttons + motionSensors + otherDevices;

  Map<String, dynamic> toJson() => {
        'lights': lights,
        'buttons': buttons,
        'motionSensors': motionSensors,
        'otherDevices': otherDevices,
        'total': total,
      };
}

class DeviceProbeResultDto {
  const DeviceProbeResultDto({
    required this.hubId,
    required this.status,
    required this.checkedAt,
    required this.tokenAvailable,
    required this.hasEncryptedToken,
    this.route,
    this.baseUrl,
    this.inventory,
    this.serverVersion,
    this.serverInstanceId,
    this.message,
  });

  final String hubId;
  final String status;
  final String? route;
  final String? baseUrl;
  final DateTime checkedAt;
  final LightBoxInventoryDto? inventory;
  final String? serverVersion;
  final String? serverInstanceId;
  final bool tokenAvailable;
  final bool hasEncryptedToken;
  final String? message;

  Map<String, dynamic> toJson() => {
        'hubId': hubId,
        'status': status,
        if (route != null) 'route': route,
        if (baseUrl != null) 'baseUrl': baseUrl,
        'checkedAt': checkedAt.toIso8601String(),
        'tokenAvailable': tokenAvailable,
        'hasEncryptedToken': hasEncryptedToken,
        if (inventory != null) 'inventory': inventory!.toJson(),
        if (serverVersion != null) 'serverVersion': serverVersion,
        if (serverInstanceId != null) 'serverInstanceId': serverInstanceId,
        if (message != null) 'message': message,
      };
}

Map<String, dynamic>? asStringMap(Object? value) {
  if (value is! Map) return null;
  return value.map((key, value) => MapEntry(key.toString(), value));
}

String? cleanString(Object? value) {
  if (value is! String) return null;
  final trimmed = value.trim();
  return trimmed.isEmpty ? null : trimmed;
}

DateTime? parseDateTime(Object? value) {
  if (value is DateTime) return value;
  if (value is! String || value.trim().isEmpty) return null;
  return DateTime.tryParse(value);
}

bool _hasUsableString(Object? value) =>
    value is String && value.trim().isNotEmpty;
