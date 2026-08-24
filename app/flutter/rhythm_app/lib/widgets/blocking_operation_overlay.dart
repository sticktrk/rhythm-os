import 'package:flutter/material.dart';

import 'solar_orbit.dart';

class BlockingOperationOverlayHandle {
  BlockingOperationOverlayHandle._(this._navigator, this._route);

  final NavigatorState _navigator;
  final Route<void> _route;
  bool _removed = false;

  void remove() {
    if (_removed) return;
    _removed = true;
    if (_navigator.mounted && _route.isActive) {
      _navigator.removeRoute(_route);
    }
  }
}

/// A synchronously inserted, non-dismissible progress surface for operations
/// that must finish before the user can safely navigate elsewhere.
BlockingOperationOverlayHandle showBlockingOperationOverlay(
  BuildContext context, {
  required String message,
}) {
  final navigator = Navigator.of(context, rootNavigator: true);
  final route = DialogRoute<void>(
    context: context,
    barrierDismissible: false,
    barrierColor: const Color(0xB3000000),
    builder: (_) => PopScope(
      key: const ValueKey('blocking-operation-overlay'),
      canPop: false,
      child: Center(
        child: Material(
          color: CelestialColors.backgroundCard,
          borderRadius: BorderRadius.circular(18),
          child: Padding(
            padding: const EdgeInsets.symmetric(
              horizontal: 28,
              vertical: 24,
            ),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                const SizedBox(
                  width: 30,
                  height: 30,
                  child: CircularProgressIndicator(
                    color: CelestialColors.sunWarm,
                    strokeWidth: 2.5,
                  ),
                ),
                const SizedBox(height: 16),
                Text(
                  message,
                  textAlign: TextAlign.center,
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 15,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    ),
  );
  navigator.push(route);
  return BlockingOperationOverlayHandle._(navigator, route);
}
