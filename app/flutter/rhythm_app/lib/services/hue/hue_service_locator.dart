/// Service locator for Hue bridge implementations.
///
/// Returns the appropriate [HueBridgeService] implementation based on
/// whether demo mode is active.
library;

import 'dart:async';

import '../demo_server_api.dart';
import 'demo_hue_bridge_service.dart';
import 'hue_bridge_service.dart';
import 'real_hue_bridge_service.dart';

/// Demo credentials for App Store review.
class DemoCredentials {
  static const email = 'REMOVED_PRIVATE_VALUE';
  static const password = 'REMOVED_PRIVATE_VALUE';
}

/// Service locator that returns appropriate [HueBridgeService] implementation.
///
/// Usage:
/// ```dart
/// // Set demo mode during sign-in
/// HueServiceLocator.setDemoMode(true);
///
/// // Get service (returns demo or real based on mode)
/// final service = HueServiceLocator.instance;
/// final rooms = await service.fetchRooms();
/// ```
class HueServiceLocator {
  HueServiceLocator._();

  static bool _isDemoMode = false;

  static final List<Future<void> Function()> _onEnabled = [];
  static final List<Future<void> Function()> _onDisabled = [];

  /// Whether demo mode is currently active.
  static bool get isDemoMode => _isDemoMode;

  /// Register a callback to run when demo mode flips to enabled.
  /// Used by [HomeProvider] to seed the demo home + server hub.
  static void onDemoEnabled(Future<void> Function() callback) {
    _onEnabled.add(callback);
  }

  /// Register a callback to run when demo mode flips to disabled.
  /// Used by [HomeProvider] to clean up demo-seeded data on sign-out.
  static void onDemoDisabled(Future<void> Function() callback) {
    _onDisabled.add(callback);
  }

  /// Set demo mode.
  ///
  /// Call with `true` when user signs in with demo credentials.
  /// Call with `false` when user signs out or signs in with real credentials.
  static void setDemoMode(bool enabled) {
    final changed = _isDemoMode != enabled;
    _isDemoMode = enabled;
    if (enabled) {
      DemoServerApi.instance.ensureSeeded();
      if (changed) {
        for (final cb in _onEnabled) {
          unawaited(cb());
        }
      }
    } else {
      // Clear demo state when exiting demo mode
      DemoServerApi.instance.reset();
      DemoHueBridgeService.instance.reset();
      if (changed) {
        for (final cb in _onDisabled) {
          unawaited(cb());
        }
      }
    }
  }

  /// Get the appropriate [HueBridgeService] implementation.
  ///
  /// Returns [DemoHueBridgeService] if demo mode is active,
  /// [RealHueBridgeService] otherwise.
  static HueBridgeService get instance {
    return _isDemoMode
        ? DemoHueBridgeService.instance
        : RealHueBridgeService.instance;
  }

  /// Get the real service directly (for configuration).
  ///
  /// Use this when you need to configure the real service even if demo
  /// mode is active (e.g., storing credentials for later use).
  static RealHueBridgeService get realInstance => RealHueBridgeService.instance;

  /// Get the demo service directly (for testing).
  static DemoHueBridgeService get demoInstance => DemoHueBridgeService.instance;
}
