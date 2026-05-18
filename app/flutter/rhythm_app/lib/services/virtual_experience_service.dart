import 'package:flutter/foundation.dart';

import 'hue/hue_service_locator.dart';

/// Tracks whether the app is currently running in the "Virtual Experience"
/// — a guided demo a prospective user can enter from the first onboarding
/// page without any hardware.
///
/// Conceptually this is a thin shell around [HueServiceLocator]'s demo
/// mode: the underlying fake rooms/devices come from the same demo
/// infrastructure used by the App Store demo account. The distinction is
/// intentional — demo mode can also be reached via the demo credentials
/// on the sign-in screen, but those users have completed onboarding and
/// shouldn't see the in-app "Exit Virtual Experience" affordance.
class VirtualExperienceService extends ChangeNotifier {
  VirtualExperienceService._();
  static final VirtualExperienceService instance =
      VirtualExperienceService._();

  bool _isActive = false;
  bool get isActive => _isActive;

  void enter() {
    if (_isActive) return;
    _isActive = true;
    HueServiceLocator.setDemoMode(true);
    notifyListeners();
  }

  void exit() {
    if (!_isActive) return;
    _isActive = false;
    HueServiceLocator.setDemoMode(false);
    notifyListeners();
  }
}
