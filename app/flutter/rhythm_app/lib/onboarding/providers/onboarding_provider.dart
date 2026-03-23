import 'package:flutter/foundation.dart';
import '../../config/feature_flags.dart';

/// Simple location data for storing coordinates.
class LocationData {
  final double latitude;
  final double longitude;

  const LocationData(this.latitude, this.longitude);
}

/// User preferences collected during onboarding.
class OnboardingPreferences {
  final int bedtimeHour;
  final int bedtimeMinute;
  final int wakeTimeHour;
  final int wakeTimeMinute;
  final LocationData? location;
  final String? locationName;
  final String? timezone;
  final bool notificationsEnabled;

  const OnboardingPreferences({
    this.bedtimeHour = 22,
    this.bedtimeMinute = 30,
    this.wakeTimeHour = 6,
    this.wakeTimeMinute = 30,
    this.location,
    this.locationName,
    this.timezone,
    this.notificationsEnabled = false,
  });

  OnboardingPreferences copyWith({
    int? bedtimeHour,
    int? bedtimeMinute,
    int? wakeTimeHour,
    int? wakeTimeMinute,
    LocationData? location,
    String? locationName,
    String? timezone,
    bool? notificationsEnabled,
  }) {
    return OnboardingPreferences(
      bedtimeHour: bedtimeHour ?? this.bedtimeHour,
      bedtimeMinute: bedtimeMinute ?? this.bedtimeMinute,
      wakeTimeHour: wakeTimeHour ?? this.wakeTimeHour,
      wakeTimeMinute: wakeTimeMinute ?? this.wakeTimeMinute,
      location: location ?? this.location,
      locationName: locationName ?? this.locationName,
      timezone: timezone ?? this.timezone,
      notificationsEnabled: notificationsEnabled ?? this.notificationsEnabled,
    );
  }
}

/// State management for onboarding flow.
class OnboardingProvider extends ChangeNotifier {
  int _currentPage = 0;
  OnboardingPreferences _preferences = const OnboardingPreferences();
  bool _isSignInMode = false;

  int get currentPage => _currentPage;
  OnboardingPreferences get preferences => _preferences;
  bool get isSignInMode => _isSignInMode;

  /// Total number of screens in the onboarding flow.
  /// 2 pages when login disabled (Welcome, Location), 3 when enabled (+ Account).
  int get totalPages => FeatureFlags.loginEnabled ? 3 : 2;

  /// Navigate to next page.
  void nextPage() {
    if (_currentPage < totalPages - 1) {
      _currentPage++;
      notifyListeners();
    }
  }

  /// Navigate to previous page.
  void previousPage() {
    if (_currentPage > 0) {
      _currentPage--;
      notifyListeners();
    }
  }

  /// Jump directly to a specific page.
  void goToPage(int page) {
    if (page >= 0 && page < totalPages) {
      _currentPage = page;
      notifyListeners();
    }
  }

  /// Enable sign-in mode (skip to account screen).
  /// No-op when login is disabled.
  void enableSignInMode() {
    if (!FeatureFlags.loginEnabled) return;
    _isSignInMode = true;
    _currentPage = 2; // Account screen
    notifyListeners();
  }

  /// Update bedtime.
  void setBedtime(int hour, int minute) {
    _preferences = _preferences.copyWith(
      bedtimeHour: hour,
      bedtimeMinute: minute,
    );
    notifyListeners();
  }

  /// Update wake time.
  void setWakeTime(int hour, int minute) {
    _preferences = _preferences.copyWith(
      wakeTimeHour: hour,
      wakeTimeMinute: minute,
    );
    notifyListeners();
  }

  /// Update location.
  void setLocation(double latitude, double longitude, String? timezone, [String? locationName]) {
    _preferences = _preferences.copyWith(
      location: LocationData(latitude, longitude),
      locationName: locationName,
      timezone: timezone,
    );
    notifyListeners();
  }

  /// Update notifications preference.
  void setNotificationsEnabled(bool enabled) {
    _preferences = _preferences.copyWith(notificationsEnabled: enabled);
    notifyListeners();
  }

  /// Reset to initial state.
  void reset() {
    _currentPage = 0;
    _preferences = const OnboardingPreferences();
    _isSignInMode = false;
    notifyListeners();
  }
}
