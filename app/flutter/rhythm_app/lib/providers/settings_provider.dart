import 'package:flutter/foundation.dart';
import 'package:package_info_plus/package_info_plus.dart';
import 'package:rhythm_core/rhythm_core.dart';
import '../services/settings_service.dart';

/// Provider for settings screen state management.
///
/// Manages:
/// - App version info
/// - Location coordinates - from HomeProvider.currentHome
/// - Sleep schedule (bedtime/wake time) - from HomeProvider.currentHome
/// - Hub configurations - from HomeProvider.currentHomeHubs
class SettingsProvider extends ChangeNotifier {
  String _appVersion = '';
  bool _isLoading = true;

  // Cached values from external sources
  Home? _currentHome;
  List<Hub> _currentHomeHubs = [];

  // Getters
  String get appVersion => _appVersion;
  bool get isLoading => _isLoading;

  /// Location from current home.
  double? get latitude => _currentHome?.location?.latitude;
  double? get longitude => _currentHome?.location?.longitude;
  String? get locationName => _currentHome?.location?.cityName;

  /// Sleep schedule from current home.
  int get bedtimeHour {
    final bedtime = _currentHome?.sleepSchedule.bedtime ?? 22.5;
    return bedtime.floor();
  }

  int get bedtimeMinute {
    final bedtime = _currentHome?.sleepSchedule.bedtime ?? 22.5;
    return ((bedtime - bedtime.floor()) * 60).round();
  }

  int get wakeTimeHour {
    final wakeTime = _currentHome?.sleepSchedule.wakeTime ?? 6.5;
    return wakeTime.floor();
  }

  int get wakeTimeMinute {
    final wakeTime = _currentHome?.sleepSchedule.wakeTime ?? 6.5;
    return ((wakeTime - wakeTime.floor()) * 60).round();
  }

  /// Hub configuration (derived from current home hubs).
  bool get haConfigured => _currentHomeHubs.any((h) => h.type == HubType.homeAssistant && h.hasCredentials);
  String get haHost {
    final haHub = _currentHomeHubs.firstWhere(
      (h) => h.type == HubType.homeAssistant,
      orElse: () => Hub.homeAssistant(
        id: '',
        homeId: '',
        name: '',
        host: '',
        token: '',
      ),
    );
    return haHub.endpoint.host;
  }

  int get haPort {
    final haHub = _currentHomeHubs.firstWhere(
      (h) => h.type == HubType.homeAssistant,
      orElse: () => Hub.homeAssistant(
        id: '',
        homeId: '',
        name: '',
        host: '',
        port: 8123,
        token: '',
      ),
    );
    return haHub.endpoint.port;
  }

  bool get hueConfigured => _currentHomeHubs.any((h) => h.type == HubType.hue && h.hasCredentials);
  String get hueBridgeIp {
    final hueHub = _currentHomeHubs.firstWhere(
      (h) => h.type == HubType.hue,
      orElse: () => Hub.hue(
        id: '',
        homeId: '',
        name: '',
        bridgeIp: '',
        appKey: '',
      ),
    );
    return hueHub.endpoint.host;
  }

  SettingsProvider() {
    loadSettings();
  }

  /// Load settings from various sources.
  Future<void> loadSettings() async {
    _isLoading = true;
    notifyListeners();

    // Load app version (includes build number)
    try {
      final packageInfo = await PackageInfo.fromPlatform();
      _appVersion = '${packageInfo.version} (${packageInfo.buildNumber})';
    } catch (e) {
      _appVersion = '1.0.0';
    }

    _isLoading = false;
    notifyListeners();
  }

  /// Update with current home data.
  ///
  /// Call this when HomeProvider changes to sync settings display.
  void updateFromHome(Home? home, List<Hub> hubs) {
    _currentHome = home;
    _currentHomeHubs = hubs;
    notifyListeners();
  }

  /// Format a time using the given format preference.
  String formatTime(int hour, int minute, {required bool use24h}) {
    return SettingsService.instance.formatTime(hour, minute, use24h: use24h);
  }

  /// Format location as place name or coordinates string.
  String formatLocation() {
    if (latitude == null || longitude == null) {
      return 'Not set';
    }
    // Prefer place name if available
    if (locationName != null && locationName!.isNotEmpty) {
      return locationName!;
    }
    return '${latitude!.toStringAsFixed(2)}deg, ${longitude!.toStringAsFixed(2)}deg';
  }

  /// Calculate sleep duration string.
  String getSleepDuration() {
    int bedtimeMinutes = bedtimeHour * 60 + bedtimeMinute;
    int wakeMinutes = wakeTimeHour * 60 + wakeTimeMinute;

    int durationMinutes = wakeMinutes - bedtimeMinutes;
    if (durationMinutes <= 0) {
      durationMinutes += 24 * 60; // Handle overnight sleep
    }

    final hours = durationMinutes ~/ 60;
    final minutes = durationMinutes % 60;

    if (minutes > 0) {
      return '${hours}h ${minutes}m';
    }
    return '${hours}h';
  }

  /// Clear Hue settings.
  ///
  /// Note: This should be called via HomeProvider.deleteHub() instead
  /// to properly remove the hub from storage and cloud.
  @Deprecated('Use HomeProvider.deleteHub() instead')
  Future<void> clearHueSettings() async {
    // No-op - hub deletion should go through HomeProvider
    notifyListeners();
  }
}
