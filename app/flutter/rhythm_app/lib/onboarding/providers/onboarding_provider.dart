import 'package:flutter/foundation.dart';
import '../../config/feature_flags.dart';

/// User preferences collected during onboarding.
class OnboardingPreferences {
  final int bedtimeHour;
  final int bedtimeMinute;
  final int wakeTimeHour;
  final int wakeTimeMinute;
  final bool notificationsEnabled;

  const OnboardingPreferences({
    this.bedtimeHour = 22,
    this.bedtimeMinute = 30,
    this.wakeTimeHour = 6,
    this.wakeTimeMinute = 30,
    this.notificationsEnabled = false,
  });

  OnboardingPreferences copyWith({
    int? bedtimeHour,
    int? bedtimeMinute,
    int? wakeTimeHour,
    int? wakeTimeMinute,
    bool? notificationsEnabled,
  }) {
    return OnboardingPreferences(
      bedtimeHour: bedtimeHour ?? this.bedtimeHour,
      bedtimeMinute: bedtimeMinute ?? this.bedtimeMinute,
      wakeTimeHour: wakeTimeHour ?? this.wakeTimeHour,
      wakeTimeMinute: wakeTimeMinute ?? this.wakeTimeMinute,
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
  /// 1 page when sign-in disabled (Welcome), 2 when enabled (+ Account).
  /// Location is collected later, during Home creation.
  int get totalPages => FeatureFlags.onboardingSignIn ? 2 : 1;

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
  /// No-op when onboarding sign-in is disabled.
  void enableSignInMode() {
    if (!FeatureFlags.onboardingSignIn) return;
    _isSignInMode = true;
    _currentPage = 1; // Account screen
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
