import '../json_parsing.dart';
import 'rhythm_capabilities.dart';
import 'rhythm_curve_config.dart';
import 'rhythm_hub_info.dart';
import 'rhythm_input_binding.dart';
import 'rhythm_review.dart';
import 'rhythm_room.dart';
import 'rhythm_scene.dart';
import 'rhythm_settings.dart';

/// Full state from the Rhythm server on connect (GET /api/state).
class RhythmHello {
  final String version;
  final String? serverInstanceId;
  final String platformType;
  final String platformContext;
  final int? listenPort;
  final List<RhythmRoom> nodes;
  final Map<String, dynamic> hub;
  final List<Map<String, dynamic>> hubs;
  final List<RhythmHubInfo> hubInfos;
  final RhythmCapabilities? capabilities;
  final Map<String, dynamic> activeProfile;
  final RhythmModeResource? mode;
  final List<RhythmModeTransitionConfig> transitions;
  final List<Map<String, dynamic>> powerSchedules;
  final List<RhythmInputBinding> inputBindings;
  final List<RhythmCurveConfig> profiles;
  final List<RhythmSceneDefinition> scenes;
  final Map<String, dynamic> location;
  final RhythmSettings? settings;
  final RhythmLightBreaker? lightBreaker;
  final RhythmLightRuntime lightRuntime;
  final bool hasLightRuntime;
  final RhythmReviewSummary review;
  final int? lastTickEpochMs;

  /// Always-present computed fade duration (accounts for auto mode).
  final int? effectiveFadeMs;

  /// Always-present computed motion timeout (accounts for auto mode).
  final int? effectiveMotionTimeoutSecs;

  const RhythmHello({
    required this.version,
    this.serverInstanceId,
    required this.platformType,
    required this.platformContext,
    this.listenPort,
    required this.nodes,
    required this.hub,
    required this.hubs,
    this.hubInfos = const [],
    this.capabilities,
    required this.activeProfile,
    this.mode,
    this.transitions = const [],
    this.powerSchedules = const [],
    this.inputBindings = const [],
    this.profiles = const [],
    this.scenes = const [],
    required this.location,
    this.settings,
    this.lightBreaker,
    this.lightRuntime = RhythmLightRuntime.rhythmAdaptive,
    this.hasLightRuntime = false,
    this.review = const RhythmReviewSummary(),
    this.lastTickEpochMs,
    this.effectiveFadeMs,
    this.effectiveMotionTimeoutSecs,
  });

  List<RhythmRoom> get rooms => nodes;

  factory RhythmHello.fromJson(Map<String, dynamic> json) {
    final settingsJson = jsonMap(json['settings']);
    final lightBreakerJson = jsonMap(json['light_breaker']);
    final rawHub = jsonMap(json['hub']) ?? const <String, dynamic>{};
    final hubsList =
        (json['hubs'] as List<dynamic>?)?.map(jsonMap).nonNulls.toList();
    final hubs = hubsList ??
        (rawHub.isNotEmpty && rawHub['type'] != 'none'
            ? [rawHub]
            : <Map<String, dynamic>>[]);
    final hub = rawHub.isNotEmpty
        ? rawHub
        : (hubs.isNotEmpty ? hubs.first : const <String, dynamic>{});
    final hubInfos = hubs.map(RhythmHubInfo.fromJson).toList(growable: false);
    final location = _normalizeLocation(json);
    final activeProfile = normalizeActiveProfile(json);
    final modeJson = jsonMap(json['mode']);
    final modeResource =
        modeJson != null ? RhythmModeResource.fromJson(modeJson) : null;
    final settings =
        settingsJson != null ? RhythmSettings.fromJson(settingsJson) : null;
    final capabilitiesJson = jsonMap(json['capabilities']);
    final reviewJson = jsonMap(json['review']);
    final profiles = ((json['profiles'] as List<dynamic>?) ?? const <dynamic>[])
        .map(jsonMap)
        .nonNulls
        .map(RhythmCurveConfig.fromJson)
        .toList();
    final scenes = ((json['scenes'] as List<dynamic>?) ?? const <dynamic>[])
        .map(jsonMap)
        .nonNulls
        .map(RhythmSceneDefinition.fromJson)
        .toList();
    final activeProfileMap = jsonMap(json['active_profile']);
    final effectiveProfile =
        jsonMap(activeProfileMap?['effective']) ?? const <String, dynamic>{};
    final activeProfileId = activeProfile['id'] as String? ??
        modeResource?.activeConfig?.activeProfileId;
    final runtimeId = json['light_runtime'] as String? ??
        json['runtime_id'] as String? ??
        (settings?.hasLightRuntime == true
            ? settings?.lightRuntime.id
            : null) ??
        (modeResource?.hasLightRuntime == true
            ? modeResource?.lightRuntime.id
            : null);
    final lightRuntime = runtimeId == null
        ? (modeResource?.lightRuntime ??
            (activeProfileId == 'expert'
                ? RhythmLightRuntime.removed-projectCircadian
                : RhythmLightRuntime.rhythmAdaptive))
        : RhythmLightRuntime.fromId(runtimeId);
    return RhythmHello(
      version: json['version'] as String? ?? '0.0.0',
      serverInstanceId: json['server_instance_id'] as String?,
      platformType: json['platform'] as String? ?? 'desktop',
      platformContext: json['context'] as String? ?? 'server',
      listenPort:
          jsonInt(json['listen_port'], preferredKeys: const ['listen_port']),
      nodes: ((json['nodes'] as List<dynamic>?) ??
              (json['rooms'] as List<dynamic>?) ??
              const [])
          .map(jsonMap)
          .nonNulls
          .map(RhythmRoom.fromJson)
          .where((r) => r.id.isNotEmpty)
          .toList(),
      hub: hub,
      hubs: hubs,
      hubInfos: hubInfos,
      capabilities: capabilitiesJson == null
          ? null
          : RhythmCapabilities.fromJson(capabilitiesJson),
      activeProfile: activeProfile,
      mode: modeResource,
      transitions:
          ((json['transitions'] as List<dynamic>?) ?? const <dynamic>[])
              .map(jsonMap)
              .nonNulls
              .map(RhythmModeTransitionConfig.fromJson)
              .toList(),
      powerSchedules:
          ((json['power_schedules'] as List<dynamic>?) ?? const <dynamic>[])
              .map(jsonMap)
              .nonNulls
              .map(Map<String, dynamic>.from)
              .toList(growable: false),
      inputBindings:
          ((json['input_bindings'] as List<dynamic>?) ?? const <dynamic>[])
              .map(jsonMap)
              .nonNulls
              .map(RhythmInputBinding.fromJson)
              .toList(),
      profiles: profiles,
      scenes: scenes,
      location: location,
      settings: settings,
      lightBreaker: lightBreakerJson != null
          ? RhythmLightBreaker.fromJson(lightBreakerJson)
          : null,
      lightRuntime: lightRuntime,
      hasLightRuntime: runtimeId != null,
      review: reviewJson == null
          ? const RhythmReviewSummary()
          : RhythmReviewSummary.fromJson(reviewJson),
      lastTickEpochMs: jsonInt(
        json['last_tick_epoch_ms'],
        preferredKeys: const ['last_tick_epoch_ms'],
      ),
      effectiveFadeMs: jsonInt(
            json['effective_fade_ms'],
            preferredKeys: const ['effective_fade_ms', 'fade_ms'],
          ) ??
          jsonInt(
            effectiveProfile['fade_ms'],
            preferredKeys: const ['fade_ms'],
          ),
      effectiveMotionTimeoutSecs: jsonInt(
            json['effective_motion_timeout_secs'],
            preferredKeys: const [
              'effective_motion_timeout_secs',
              'motion_timeout_secs',
            ],
          ) ??
          jsonInt(
            effectiveProfile['motion_timeout_secs'],
            preferredKeys: const ['motion_timeout_secs'],
          ),
    );
  }
}

Map<String, dynamic> _normalizeLocation(Map<String, dynamic> json) {
  final location = Map<String, dynamic>.from(
    jsonMap(json['location']) ?? const <String, dynamic>{},
  );
  final currentLocalTime =
      location['current_local_time'] ?? json['current_time'];
  if (currentLocalTime != null) {
    location['current_local_time'] = currentLocalTime;
  }
  return location;
}
