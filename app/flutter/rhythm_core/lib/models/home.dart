import 'package:hive/hive.dart';
import '../src/rust/api/dto/curve.dart' show CurveConfigDto;
import 'config_state.dart' show RawConfig;

part 'home.g.dart';

/// Geographic location for a home.
@HiveType(typeId: 10)
class HomeLocation {
  @HiveField(0)
  final double latitude;

  @HiveField(1)
  final double longitude;

  @HiveField(2)
  final String? cityName;

  const HomeLocation({
    required this.latitude,
    required this.longitude,
    this.cityName,
  });

  factory HomeLocation.fromJson(Map<String, dynamic> json) {
    return HomeLocation(
      latitude: (json['latitude'] as num).toDouble(),
      longitude: (json['longitude'] as num).toDouble(),
      cityName: json['cityName'] as String?,
    );
  }

  Map<String, dynamic> toJson() {
    return {
      'latitude': latitude,
      'longitude': longitude,
      if (cityName != null) 'cityName': cityName,
    };
  }

  HomeLocation copyWith({
    double? latitude,
    double? longitude,
    String? cityName,
  }) {
    return HomeLocation(
      latitude: latitude ?? this.latitude,
      longitude: longitude ?? this.longitude,
      cityName: cityName ?? this.cityName,
    );
  }
}

/// Sleep schedule configuration for adaptive lighting behavior.
@HiveType(typeId: 11)
class SleepSchedule {
  /// Bedtime in 24-hour format (e.g., 22.5 = 10:30 PM)
  @HiveField(0)
  final double bedtime;

  /// Wake time in 24-hour format (e.g., 6.5 = 6:30 AM)
  @HiveField(1)
  final double wakeTime;

  /// Whether the sleep schedule is enabled
  @HiveField(2)
  final bool enabled;

  const SleepSchedule({
    required this.bedtime,
    required this.wakeTime,
    this.enabled = true,
  });

  factory SleepSchedule.defaults() {
    return const SleepSchedule(
      bedtime: 22.0, // 10:00 PM
      wakeTime: 6.5, // 6:30 AM
      enabled: true,
    );
  }

  factory SleepSchedule.fromJson(Map<String, dynamic> json) {
    return SleepSchedule(
      bedtime: (json['bedtime'] as num).toDouble(),
      wakeTime: (json['wakeTime'] as num).toDouble(),
      enabled: json['enabled'] as bool? ?? true,
    );
  }

  Map<String, dynamic> toJson() {
    return {
      'bedtime': bedtime,
      'wakeTime': wakeTime,
      'enabled': enabled,
    };
  }

  SleepSchedule copyWith({
    double? bedtime,
    double? wakeTime,
    bool? enabled,
  }) {
    return SleepSchedule(
      bedtime: bedtime ?? this.bedtime,
      wakeTime: wakeTime ?? this.wakeTime,
      enabled: enabled ?? this.enabled,
    );
  }
}

/// A Home represents a physical location containing lighting hubs.
///
/// Each user can have multiple homes, and homes can be shared with multiple users.
@HiveType(typeId: 12)
class Home {
  /// Firestore document ID
  @HiveField(0)
  final String id;

  /// Display name (e.g., "My Home", "Beach House")
  @HiveField(1)
  final String name;

  /// Firebase UID of the home owner
  @HiveField(2)
  final String ownerId;

  /// Firebase UIDs of members who have access (includes owner)
  @HiveField(3)
  final List<String> memberIds;

  /// Geographic location for solar calculations
  @HiveField(4)
  final HomeLocation? location;

  /// Sleep schedule for adaptive lighting
  @HiveField(5)
  final SleepSchedule sleepSchedule;

  /// Default curve configuration for this home
  /// Stored as JSON map since CurveConfigDto is FRB-generated
  @HiveField(6)
  final Map<String, dynamic>? curveConfigJson;

  /// Timezone identifier (e.g., "America/New_York")
  @HiveField(7)
  final String? timezone;

  @HiveField(8)
  final DateTime createdAt;

  @HiveField(9)
  final DateTime updatedAt;

  /// Flag for sync status - true if pending upload to Firestore
  @HiveField(10)
  final bool pendingSync;

  const Home({
    required this.id,
    required this.name,
    required this.ownerId,
    required this.memberIds,
    this.location,
    required this.sleepSchedule,
    this.curveConfigJson,
    this.timezone,
    required this.createdAt,
    required this.updatedAt,
    this.pendingSync = false,
  });

  /// Create a new Home with default values.
  factory Home.create({
    required String id,
    required String name,
    required String ownerId,
    HomeLocation? location,
    SleepSchedule? sleepSchedule,
    CurveConfigDto? curveConfig,
    String? timezone,
  }) {
    final now = DateTime.now();
    return Home(
      id: id,
      name: name,
      ownerId: ownerId,
      memberIds: [ownerId],
      location: location,
      sleepSchedule: sleepSchedule ?? SleepSchedule.defaults(),
      curveConfigJson: curveConfig != null ? _curveConfigToJson(curveConfig) : null,
      timezone: timezone,
      createdAt: now,
      updatedAt: now,
      pendingSync: true,
    );
  }

  factory Home.fromJson(Map<String, dynamic> json) {
    return Home(
      id: json['id'] as String,
      name: json['name'] as String,
      ownerId: json['ownerId'] as String,
      memberIds: (json['memberIds'] as List<dynamic>).cast<String>(),
      location: json['location'] != null
          ? HomeLocation.fromJson(json['location'] as Map<String, dynamic>)
          : null,
      sleepSchedule: json['sleepSchedule'] != null
          ? SleepSchedule.fromJson(json['sleepSchedule'] as Map<String, dynamic>)
          : SleepSchedule.defaults(),
      curveConfigJson: json['curveConfig'] as Map<String, dynamic>?,
      timezone: json['timezone'] as String?,
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
      'name': name,
      'ownerId': ownerId,
      'memberIds': memberIds,
      if (location != null) 'location': location!.toJson(),
      'sleepSchedule': sleepSchedule.toJson(),
      if (curveConfigJson != null) 'curveConfig': curveConfigJson,
      if (timezone != null) 'timezone': timezone,
      'createdAt': createdAt.toIso8601String(),
      'updatedAt': updatedAt.toIso8601String(),
      'pendingSync': pendingSync,
    };
  }

  /// Convert to Firestore-friendly format (no local-only fields).
  Map<String, dynamic> toFirestore() {
    return {
      'name': name,
      'ownerId': ownerId,
      'memberIds': memberIds,
      if (location != null) 'location': location!.toJson(),
      'sleepSchedule': sleepSchedule.toJson(),
      if (curveConfigJson != null) 'curveConfig': curveConfigJson,
      if (timezone != null) 'timezone': timezone,
      'createdAt': createdAt.toIso8601String(),
      'updatedAt': updatedAt.toIso8601String(),
    };
  }

  /// Convert to Supabase-friendly format (snake_case keys, no local-only fields).
  Map<String, dynamic> toSupabase() {
    return {
      'id': id,
      'name': name,
      'owner_id': ownerId,
      'member_ids': memberIds,
      if (location != null) 'location': location!.toJson(),
      'sleep_schedule': sleepSchedule.toJson(),
      if (curveConfigJson != null) 'curve_config': curveConfigJson,
      if (timezone != null) 'timezone': timezone,
      'created_at': createdAt.toUtc().toIso8601String(),
      'updated_at': updatedAt.toUtc().toIso8601String(),
    };
  }

  /// Create a Home from a Supabase row (snake_case keys).
  factory Home.fromSupabase(Map<String, dynamic> row) {
    return Home(
      id: row['id'] as String,
      name: row['name'] as String,
      ownerId: row['owner_id'] as String,
      memberIds: List<String>.from(row['member_ids'] ?? []),
      location: row['location'] != null
          ? HomeLocation.fromJson(row['location'] as Map<String, dynamic>)
          : null,
      sleepSchedule: row['sleep_schedule'] != null
          ? SleepSchedule.fromJson(row['sleep_schedule'] as Map<String, dynamic>)
          : SleepSchedule.defaults(),
      curveConfigJson: row['curve_config'] as Map<String, dynamic>?,
      timezone: row['timezone'] as String?,
      createdAt: DateTime.parse(row['created_at'] as String),
      updatedAt: DateTime.parse(row['updated_at'] as String),
      pendingSync: false, // Cloud data is not pending sync
    );
  }

  /// Get the curve config as a CurveConfigDto (for Rust interop).
  CurveConfigDto? get curveConfig {
    if (curveConfigJson == null) return null;
    return _curveConfigFromJson(curveConfigJson!);
  }

  /// Get the curve config as RawConfig (for Flutter use).
  RawConfig? get rawCurveConfig {
    if (curveConfigJson == null) return null;
    return RawConfig.fromJson(curveConfigJson!);
  }

  Home copyWith({
    String? id,
    String? name,
    String? ownerId,
    List<String>? memberIds,
    HomeLocation? location,
    SleepSchedule? sleepSchedule,
    Map<String, dynamic>? curveConfigJson,
    String? timezone,
    DateTime? createdAt,
    DateTime? updatedAt,
    bool? pendingSync,
  }) {
    return Home(
      id: id ?? this.id,
      name: name ?? this.name,
      ownerId: ownerId ?? this.ownerId,
      memberIds: memberIds ?? this.memberIds,
      location: location ?? this.location,
      sleepSchedule: sleepSchedule ?? this.sleepSchedule,
      curveConfigJson: curveConfigJson ?? this.curveConfigJson,
      timezone: timezone ?? this.timezone,
      createdAt: createdAt ?? this.createdAt,
      updatedAt: updatedAt ?? this.updatedAt,
      pendingSync: pendingSync ?? this.pendingSync,
    );
  }

  /// Create a copy with updated curve config from CurveConfigDto.
  Home withCurveConfig(CurveConfigDto config) {
    return copyWith(
      curveConfigJson: _curveConfigToJson(config),
      updatedAt: DateTime.now(),
      pendingSync: true,
    );
  }

  /// Mark this home as needing sync.
  Home markPendingSync() {
    return copyWith(
      updatedAt: DateTime.now(),
      pendingSync: true,
    );
  }

  /// Mark this home as synced.
  Home markSynced() {
    return copyWith(pendingSync: false);
  }
}

/// Helper to convert CurveConfigDto to JSON.
Map<String, dynamic> _curveConfigToJson(CurveConfigDto config) {
  return {
    'min_color_temp': config.minColorTemp,
    'max_color_temp': config.maxColorTemp,
    'min_brightness': config.minBrightness,
    'max_brightness': config.maxBrightness,
    'width_left_bri': config.widthLeftBri,
    'width_right_bri': config.widthRightBri,
    'width_left_cct': config.widthLeftCct,
    'width_right_cct': config.widthRightCct,
    'shape_p': config.shapeP,
    'max_dim_steps': config.maxDimSteps,
    'fade_ms': config.fadeMs,
    'motion_timeout_secs': config.motionTimeoutSecs,
  };
}

/// Helper to convert JSON to CurveConfigDto.
CurveConfigDto _curveConfigFromJson(Map<String, dynamic> json) {
  final d = CurveConfigDto.default_();
  return CurveConfigDto(
    minColorTemp: (json['min_color_temp'] as num?)?.toInt() ?? d.minColorTemp,
    maxColorTemp: (json['max_color_temp'] as num?)?.toInt() ?? d.maxColorTemp,
    minBrightness: (json['min_brightness'] as num?)?.toInt() ?? d.minBrightness,
    maxBrightness: (json['max_brightness'] as num?)?.toInt() ?? d.maxBrightness,
    widthLeftBri: (json['width_left_bri'] as num?)?.toDouble() ?? d.widthLeftBri,
    widthRightBri: (json['width_right_bri'] as num?)?.toDouble() ?? d.widthRightBri,
    widthLeftCct: (json['width_left_cct'] as num?)?.toDouble() ?? d.widthLeftCct,
    widthRightCct: (json['width_right_cct'] as num?)?.toDouble() ?? d.widthRightCct,
    shapeP: (json['shape_p'] as num?)?.toDouble() ?? d.shapeP,
    maxDimSteps: (json['max_dim_steps'] as num?)?.toInt() ?? d.maxDimSteps,
    fadeMs: (json['fade_ms'] as num?)?.toInt() ?? d.fadeMs,
    motionTimeoutSecs: (json['motion_timeout_secs'] as num?)?.toInt() ?? d.motionTimeoutSecs,
  );
}
