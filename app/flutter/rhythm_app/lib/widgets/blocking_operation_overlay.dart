import 'package:flutter/material.dart';

import 'solar_orbit.dart';

/// A synchronously inserted, non-dismissible progress surface for operations
/// that must finish before the user can safely navigate elsewhere.
OverlayEntry showBlockingOperationOverlay(
  BuildContext context, {
  required String message,
}) {
  final entry = OverlayEntry(
    builder: (_) => Stack(
      key: const ValueKey('blocking-operation-overlay'),
      children: [
        const ModalBarrier(
          dismissible: false,
          color: Color(0xB3000000),
        ),
        Center(
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
      ],
    ),
  );
  Overlay.of(context, rootOverlay: true).insert(entry);
  return entry;
}
