import '../json_parsing.dart';

enum RhythmHueRoomAuthorityOwner {
  unreviewed,
  hue,
  rhythm;

  String get wireValue => name;

  static RhythmHueRoomAuthorityOwner fromWire(Object? value) {
    return switch (value) {
      'hue' => RhythmHueRoomAuthorityOwner.hue,
      'rhythm' => RhythmHueRoomAuthorityOwner.rhythm,
      _ => RhythmHueRoomAuthorityOwner.unreviewed,
    };
  }
}

class RhythmHueRoomAuthority {
  final String roomId;
  final String name;
  final RhythmHueRoomAuthorityOwner owner;
  final bool rhythmAutomationEnabled;

  const RhythmHueRoomAuthority({
    required this.roomId,
    required this.name,
    required this.owner,
    required this.rhythmAutomationEnabled,
  });

  factory RhythmHueRoomAuthority.fromJson(Map<String, dynamic> json) {
    return RhythmHueRoomAuthority(
      roomId: json['room_id'] as String? ?? '',
      name: json['name'] as String? ?? '',
      owner: RhythmHueRoomAuthorityOwner.fromWire(json['owner']),
      rhythmAutomationEnabled:
          json['rhythm_automation_enabled'] as bool? ?? false,
    );
  }
}

class RhythmHueBridgeAuthority {
  final String address;
  final String revision;
  final String takeoverScope;
  final bool bridgeTakeoverRequested;
  final bool topologySyncEnabled;
  final String topologySyncStatus;
  final int topologySyncRoomCount;
  final int topologySyncLightCount;
  final List<RhythmHueRoomAuthority> rooms;

  const RhythmHueBridgeAuthority({
    required this.address,
    required this.revision,
    required this.takeoverScope,
    required this.bridgeTakeoverRequested,
    this.topologySyncEnabled = false,
    this.topologySyncStatus = 'disabled',
    this.topologySyncRoomCount = 0,
    this.topologySyncLightCount = 0,
    required this.rooms,
  });

  factory RhythmHueBridgeAuthority.fromJson(Map<String, dynamic> json) {
    return RhythmHueBridgeAuthority(
      address: json['address'] as String? ?? '',
      revision: json['revision'] as String? ?? '',
      takeoverScope: json['takeover_scope'] as String? ?? 'bridge',
      bridgeTakeoverRequested:
          json['bridge_takeover_requested'] as bool? ?? false,
      topologySyncEnabled: json['topology_sync_enabled'] as bool? ?? false,
      topologySyncStatus:
          json['topology_sync_status'] as String? ?? 'disabled',
      topologySyncRoomCount: jsonInt(
            json['topology_sync_room_count'],
            preferredKeys: const ['topology_sync_room_count'],
          ) ??
          0,
      topologySyncLightCount: jsonInt(
            json['topology_sync_light_count'],
            preferredKeys: const ['topology_sync_light_count'],
          ) ??
          0,
      rooms: ((json['rooms'] as List<dynamic>?) ?? const [])
          .map(jsonMap)
          .nonNulls
          .map(RhythmHueRoomAuthority.fromJson)
          .where((room) => room.roomId.isNotEmpty)
          .toList(growable: false),
    );
  }
}

class RhythmHueAuthority {
  final int schemaVersion;
  final List<RhythmHueBridgeAuthority> bridges;

  const RhythmHueAuthority({
    required this.schemaVersion,
    required this.bridges,
  });

  factory RhythmHueAuthority.fromJson(Map<String, dynamic> json) {
    return RhythmHueAuthority(
      schemaVersion: jsonInt(
            json['schema_version'],
            preferredKeys: const ['schema_version'],
          ) ??
          0,
      bridges: ((json['bridges'] as List<dynamic>?) ?? const [])
          .map(jsonMap)
          .nonNulls
          .map(RhythmHueBridgeAuthority.fromJson)
          .where((bridge) => bridge.address.isNotEmpty)
          .toList(growable: false),
    );
  }
}
