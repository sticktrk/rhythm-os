import 'rhythm_room.dart';
import 'rhythm_settings.dart';

/// Full state from the Rhythm server on connect (GET /api/state).
class RhythmHello {
  final String version;
  final String platformType;
  final String platformContext;
  final int? listenPort;
  final List<RhythmRoom> rooms;
  final Map<String, dynamic> hub;
  final List<Map<String, dynamic>> hubs;
  final Map<String, dynamic> config;
  final Map<String, dynamic> location;
  final RhythmSettings? settings;

  const RhythmHello({
    required this.version,
    required this.platformType,
    required this.platformContext,
    this.listenPort,
    required this.rooms,
    required this.hub,
    required this.hubs,
    required this.config,
    required this.location,
    this.settings,
  });

  factory RhythmHello.fromJson(Map<String, dynamic> json) {
    final settingsJson = json['settings'] as Map<String, dynamic>?;
    final hub = json['hub'] as Map<String, dynamic>? ?? {};
    final hubsList = (json['hubs'] as List<dynamic>?)
        ?.map((h) => h as Map<String, dynamic>)
        .toList();
    final hubs = hubsList ??
        (hub.isNotEmpty && hub['type'] != 'none'
            ? [hub]
            : <Map<String, dynamic>>[]);
    return RhythmHello(
      version: json['version'] as String? ?? '0.0.0',
      platformType: json['platform'] as String? ?? 'desktop',
      platformContext: json['context'] as String? ?? 'server',
      listenPort: (json['listen_port'] as num?)?.toInt(),
      rooms: (json['rooms'] as List<dynamic>?)
              ?.map((r) => RhythmRoom.fromJson(r as Map<String, dynamic>))
              .where((r) => r.id.isNotEmpty)
              .toList() ??
          [],
      hub: hub,
      hubs: hubs,
      config: json['config'] as Map<String, dynamic>? ?? {},
      location: json['location'] as Map<String, dynamic>? ?? {},
      settings:
          settingsJson != null ? RhythmSettings.fromJson(settingsJson) : null,
    );
  }
}
