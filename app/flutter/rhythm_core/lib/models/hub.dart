import 'package:hive/hive.dart';

part 'hub.g.dart';

/// Type of hub/controller.
@HiveType(typeId: 20)
enum HubType {
  @HiveField(0)
  homeAssistant,

  @HiveField(1)
  hue,

  @HiveField(2)
  server,
}

/// Network endpoint configuration for a hub.
@HiveType(typeId: 21)
class HubEndpoint {
  /// Hostname or IP address
  @HiveField(0)
  final String host;

  /// Port number
  @HiveField(1)
  final int port;

  /// Whether to use SSL/TLS
  @HiveField(2)
  final bool useSsl;

  const HubEndpoint({
    required this.host,
    required this.port,
    this.useSsl = false,
  });

  factory HubEndpoint.fromJson(Map<String, dynamic> json) {
    return HubEndpoint(
      host: json['host'] as String,
      port: json['port'] as int,
      useSsl: json['useSsl'] as bool? ?? false,
    );
  }

  Map<String, dynamic> toJson() {
    return {
      'host': host,
      'port': port,
      'useSsl': useSsl,
    };
  }

  /// Get the base URL for HTTP requests.
  String get baseUrl {
    final scheme = useSsl ? 'https' : 'http';
    return '$scheme://$host:$port';
  }

  /// Get the WebSocket URL.
  String get wsUrl {
    final scheme = useSsl ? 'wss' : 'ws';
    return '$scheme://$host:$port';
  }

  HubEndpoint copyWith({
    String? host,
    int? port,
    bool? useSsl,
  }) {
    return HubEndpoint(
      host: host ?? this.host,
      port: port ?? this.port,
      useSsl: useSsl ?? this.useSsl,
    );
  }

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is HubEndpoint &&
          runtimeType == other.runtimeType &&
          host == other.host &&
          port == other.port &&
          useSsl == other.useSsl;

  @override
  int get hashCode => host.hashCode ^ port.hashCode ^ useSsl.hashCode;
}

/// A Hub represents a lighting controller (Home Assistant, Hue Bridge, ESP32).
///
/// Hubs belong to a Home and can have credentials stored for cloud sync.
/// Credentials are protected by Firestore security rules (only memberIds can access).
@HiveType(typeId: 22)
class Hub {
  /// Firestore document ID
  @HiveField(0)
  final String id;

  /// Parent home ID
  @HiveField(1)
  final String homeId;

  /// Type of hub
  @HiveField(2)
  final HubType type;

  /// Display name (e.g., "Living Room Hue", "Main Home Assistant")
  @HiveField(3)
  final String name;

  /// Network endpoint configuration
  @HiveField(4)
  final HubEndpoint endpoint;

  /// Whether this hub is enabled
  @HiveField(5)
  final bool enabled;

  /// Whether this hub requires credentials (false for ESP32, true for HA/Hue)
  @HiveField(6)
  final bool requiresCredentials;

  /// Authentication token (HA long-lived token or Hue app key)
  /// Stored in Firestore, protected by security rules
  @HiveField(7)
  final String? token;

  /// Last successful connection timestamp
  @HiveField(8)
  final DateTime? lastConnected;

  @HiveField(9)
  final DateTime createdAt;

  @HiveField(10)
  final DateTime updatedAt;

  /// Flag for sync status - true if pending upload to Firestore
  @HiveField(11)
  final bool pendingSync;

  const Hub({
    required this.id,
    required this.homeId,
    required this.type,
    required this.name,
    required this.endpoint,
    this.enabled = true,
    required this.requiresCredentials,
    this.token,
    this.lastConnected,
    required this.createdAt,
    required this.updatedAt,
    this.pendingSync = false,
  });

  /// Create a new Hub with default values.
  factory Hub.create({
    required String id,
    required String homeId,
    required HubType type,
    required String name,
    required HubEndpoint endpoint,
    bool enabled = true,
    String? token,
  }) {
    final now = DateTime.now();
    return Hub(
      id: id,
      homeId: homeId,
      type: type,
      name: name,
      endpoint: endpoint,
      enabled: enabled,
      requiresCredentials: type != HubType.server,
      token: token,
      lastConnected: null,
      createdAt: now,
      updatedAt: now,
      pendingSync: true,
    );
  }

  /// Create a Home Assistant hub.
  factory Hub.homeAssistant({
    required String id,
    required String homeId,
    required String name,
    required String host,
    int port = 8123,
    bool useSsl = false,
    required String token,
  }) {
    return Hub.create(
      id: id,
      homeId: homeId,
      type: HubType.homeAssistant,
      name: name,
      endpoint: HubEndpoint(host: host, port: port, useSsl: useSsl),
      token: token,
    );
  }

  /// Create a Philips Hue hub.
  factory Hub.hue({
    required String id,
    required String homeId,
    required String name,
    required String bridgeIp,
    required String appKey,
  }) {
    return Hub.create(
      id: id,
      homeId: homeId,
      type: HubType.hue,
      name: name,
      endpoint: HubEndpoint(host: bridgeIp, port: 443, useSsl: true),
      token: appKey,
    );
  }

  /// Create a server hub (rhythm-server, HA addon, or ESP32).
  factory Hub.server({
    required String id,
    required String homeId,
    required String name,
    required String host,
    int port = 54448,
    String? token,
  }) {
    return Hub.create(
      id: id,
      homeId: homeId,
      type: HubType.server,
      name: name,
      endpoint: HubEndpoint(host: host, port: port, useSsl: false),
      token: token,
    );
  }

  factory Hub.fromJson(Map<String, dynamic> json) {
    return Hub(
      id: json['id'] as String,
      homeId: json['homeId'] as String,
      type: HubType.values.firstWhere(
        (t) => t.name == json['type'],
        orElse: () => json['type'] == 'esp32' ? HubType.server : HubType.homeAssistant,
      ),
      name: json['name'] as String,
      endpoint: HubEndpoint.fromJson(json['endpoint'] as Map<String, dynamic>),
      enabled: json['enabled'] as bool? ?? true,
      requiresCredentials: json['requiresCredentials'] as bool? ?? true,
      token: json['token'] as String?,
      lastConnected: json['lastConnected'] != null
          ? (json['lastConnected'] is DateTime
              ? json['lastConnected'] as DateTime
              : DateTime.parse(json['lastConnected'] as String))
          : null,
      createdAt: json['createdAt'] is DateTime
          ? json['createdAt'] as DateTime
          : DateTime.parse(json['createdAt'] as String),
      updatedAt: json['updatedAt'] is DateTime
          ? json['updatedAt'] as DateTime
          : DateTime.parse(json['updatedAt'] as String),
      pendingSync: json['pendingSync'] as bool? ?? false,
    );
  }

  Map<String, dynamic> toJson() {
    return {
      'id': id,
      'homeId': homeId,
      'type': type.name,
      'name': name,
      'endpoint': endpoint.toJson(),
      'enabled': enabled,
      'requiresCredentials': requiresCredentials,
      if (token != null) 'token': token,
      if (lastConnected != null) 'lastConnected': lastConnected!.toIso8601String(),
      'createdAt': createdAt.toIso8601String(),
      'updatedAt': updatedAt.toIso8601String(),
      'pendingSync': pendingSync,
    };
  }

  /// Convert to Firestore-friendly format (no local-only fields).
  Map<String, dynamic> toFirestore() {
    return {
      'homeId': homeId,
      'type': type.name,
      'name': name,
      'endpoint': endpoint.toJson(),
      'enabled': enabled,
      'requiresCredentials': requiresCredentials,
      if (token != null) 'token': token,
      if (lastConnected != null) 'lastConnected': lastConnected!.toIso8601String(),
      'createdAt': createdAt.toIso8601String(),
      'updatedAt': updatedAt.toIso8601String(),
    };
  }

  /// Convert to Supabase-friendly format (snake_case keys, no local-only fields).
  Map<String, dynamic> toSupabase() {
    return {
      'id': id,
      'home_id': homeId,
      'type': type.name,
      'name': name,
      'endpoint': endpoint.toJson(),
      'enabled': enabled,
      if (token != null) 'token': token,
      if (lastConnected != null) 'last_connected': lastConnected!.toUtc().toIso8601String(),
      'created_at': createdAt.toUtc().toIso8601String(),
      'updated_at': updatedAt.toUtc().toIso8601String(),
    };
  }

  /// Create a Hub from a Supabase row (snake_case keys).
  factory Hub.fromSupabase(Map<String, dynamic> row) {
    return Hub(
      id: row['id'] as String,
      homeId: row['home_id'] as String,
      type: HubType.values.firstWhere(
        (t) => t.name == row['type'],
        orElse: () => row['type'] == 'esp32' ? HubType.server : HubType.homeAssistant,
      ),
      name: row['name'] as String,
      endpoint: HubEndpoint.fromJson(row['endpoint'] as Map<String, dynamic>),
      enabled: row['enabled'] as bool? ?? true,
      requiresCredentials: row['type'] != 'esp32' && row['type'] != 'server',
      token: row['token'] as String?,
      lastConnected: row['last_connected'] != null
          ? DateTime.parse(row['last_connected'] as String)
          : null,
      createdAt: DateTime.parse(row['created_at'] as String),
      updatedAt: DateTime.parse(row['updated_at'] as String),
      pendingSync: false, // Cloud data is not pending sync
    );
  }

  Hub copyWith({
    String? id,
    String? homeId,
    HubType? type,
    String? name,
    HubEndpoint? endpoint,
    bool? enabled,
    bool? requiresCredentials,
    String? token,
    DateTime? lastConnected,
    DateTime? createdAt,
    DateTime? updatedAt,
    bool? pendingSync,
  }) {
    return Hub(
      id: id ?? this.id,
      homeId: homeId ?? this.homeId,
      type: type ?? this.type,
      name: name ?? this.name,
      endpoint: endpoint ?? this.endpoint,
      enabled: enabled ?? this.enabled,
      requiresCredentials: requiresCredentials ?? this.requiresCredentials,
      token: token ?? this.token,
      lastConnected: lastConnected ?? this.lastConnected,
      createdAt: createdAt ?? this.createdAt,
      updatedAt: updatedAt ?? this.updatedAt,
      pendingSync: pendingSync ?? this.pendingSync,
    );
  }

  /// Update last connected timestamp.
  Hub markConnected() {
    return copyWith(
      lastConnected: DateTime.now(),
      updatedAt: DateTime.now(),
      pendingSync: true,
    );
  }

  /// Mark this hub as needing sync.
  Hub markPendingSync() {
    return copyWith(
      updatedAt: DateTime.now(),
      pendingSync: true,
    );
  }

  /// Mark this hub as synced.
  Hub markSynced() {
    return copyWith(pendingSync: false);
  }

  /// Whether this hub has valid credentials configured.
  bool get hasCredentials => !requiresCredentials || (token != null && token!.isNotEmpty);

  /// Get the display type name.
  String get typeName {
    switch (type) {
      case HubType.homeAssistant:
        return 'Home Assistant';
      case HubType.hue:
        return 'Philips Hue';
      case HubType.server:
        return 'Rhythm Server';
    }
  }

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is Hub &&
          runtimeType == other.runtimeType &&
          id == other.id &&
          homeId == other.homeId;

  @override
  int get hashCode => id.hashCode ^ homeId.hashCode;
}
