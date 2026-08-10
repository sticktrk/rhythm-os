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
  final List<RhythmHueRoomAuthority> rooms;

  const RhythmHueBridgeAuthority({
    required this.address,
    required this.revision,
    required this.takeoverScope,
    required this.bridgeTakeoverRequested,
    required this.rooms,
  });

  factory RhythmHueBridgeAuthority.fromJson(Map<String, dynamic> json) {
    return RhythmHueBridgeAuthority(
      address: json['address'] as String? ?? '',
      revision: json['revision'] as String? ?? '',
      takeoverScope: json['takeover_scope'] as String? ?? 'bridge',
      bridgeTakeoverRequested:
          json['bridge_takeover_requested'] as bool? ?? false,
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
