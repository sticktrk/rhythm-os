import 'package:hive/hive.dart';

part 'app_settings.g.dart';

/// Device-specific application settings.
///
/// These settings are local to the device and should NOT sync to Firestore.
/// Location, sleep schedule, and timezone are stored in the Home model
/// (which does sync).
@HiveType(typeId: 40)
class AppSettings {
  /// Whether to use 24-hour time format.
  @HiveField(0)
  final bool use24HourFormat;

  /// Whether onboarding has been completed.
  @HiveField(1)
  final bool onboardingComplete;

  /// Whether push notifications are enabled.
  @HiveField(2)
  final bool notificationsEnabled;

  /// Whether Hue SSE (Server-Sent Events) is enabled.
  @HiveField(3)
  final bool hueSseEnabled;

  /// Cached Hue device registry JSON for SSE room mappings.
  @HiveField(4)
  final String? hueDeviceRegistryJson;

  /// Cached curve config JSON (local-only API cache).
  @HiveField(5)
  final String? curveConfigJson;

  /// Cached runner state JSON (room/runner state).
  @HiveField(6)
  final String? runnerStateJson;

  /// Whether the rhythm mode entry warning has been dismissed.
  @HiveField(7)
  final bool rhythmWarningDismissed;

  /// Cached Hue room-to-grouped_light mapping JSON.
  @HiveField(8)
  final String? hueGroupedLightMapJson;

  /// Electricity rate in currency per kWh (e.g. 0.12 for $0.12/kWh).
  @HiveField(9)
  final double? electricityRate;

  const AppSettings({
    this.use24HourFormat = false,
    this.onboardingComplete = false,
    this.notificationsEnabled = false,
    this.hueSseEnabled = true,
    this.hueDeviceRegistryJson,
    this.curveConfigJson,
    this.runnerStateJson,
    this.rhythmWarningDismissed = false,
    this.hueGroupedLightMapJson,
    this.electricityRate,
  });

  /// Create default settings.
  factory AppSettings.defaults() {
    return const AppSettings();
  }

  AppSettings copyWith({
    bool? use24HourFormat,
    bool? onboardingComplete,
    bool? notificationsEnabled,
    bool? hueSseEnabled,
    String? hueDeviceRegistryJson,
    String? curveConfigJson,
    String? runnerStateJson,
    bool? rhythmWarningDismissed,
    String? hueGroupedLightMapJson,
    double? electricityRate,
  }) {
    return AppSettings(
      use24HourFormat: use24HourFormat ?? this.use24HourFormat,
      onboardingComplete: onboardingComplete ?? this.onboardingComplete,
      notificationsEnabled: notificationsEnabled ?? this.notificationsEnabled,
      hueSseEnabled: hueSseEnabled ?? this.hueSseEnabled,
      hueDeviceRegistryJson: hueDeviceRegistryJson ?? this.hueDeviceRegistryJson,
      curveConfigJson: curveConfigJson ?? this.curveConfigJson,
      runnerStateJson: runnerStateJson ?? this.runnerStateJson,
      rhythmWarningDismissed: rhythmWarningDismissed ?? this.rhythmWarningDismissed,
      hueGroupedLightMapJson: hueGroupedLightMapJson ?? this.hueGroupedLightMapJson,
      electricityRate: electricityRate ?? this.electricityRate,
    );
  }

  /// Clear a specific JSON field by setting it to null.
  AppSettings clearField({
    bool clearHueDeviceRegistryJson = false,
    bool clearCurveConfigJson = false,
    bool clearRunnerStateJson = false,
    bool clearHueGroupedLightMapJson = false,
  }) {
    return AppSettings(
      use24HourFormat: use24HourFormat,
      onboardingComplete: onboardingComplete,
      notificationsEnabled: notificationsEnabled,
      hueSseEnabled: hueSseEnabled,
      hueDeviceRegistryJson: clearHueDeviceRegistryJson ? null : hueDeviceRegistryJson,
      curveConfigJson: clearCurveConfigJson ? null : curveConfigJson,
      runnerStateJson: clearRunnerStateJson ? null : runnerStateJson,
      rhythmWarningDismissed: rhythmWarningDismissed,
      hueGroupedLightMapJson: clearHueGroupedLightMapJson ? null : hueGroupedLightMapJson,
      electricityRate: electricityRate,
    );
  }
}
