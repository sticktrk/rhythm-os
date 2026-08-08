import 'package:flutter/material.dart';

import '../services/analytics_service.dart';
import '../services/hue_ble_auto_discovery_service.dart';
import 'solar_orbit.dart';

Future<bool> showNearbyHueBlePrompt(
  BuildContext context, {
  required String source,
  required HueBleDiscoveryResult discovery,
}) async {
  if (!discovery.found || !context.mounted) return false;

  final accepted = await showDialog<bool>(
        context: context,
        builder: (dialogContext) => AlertDialog(
          key: const ValueKey('nearby-hue-ble-prompt'),
          backgroundColor: CelestialColors.backgroundCard,
          icon: const Icon(
            Icons.bluetooth_searching_rounded,
            color: Color(0xFFFFB900),
            size: 32,
          ),
          title: const Text(
            'Nearby bulb found',
            style: TextStyle(color: CelestialColors.textPrimary),
          ),
          content: Text(
            discovery.deviceCount > 1
                ? 'Rhythm found nearby Hue Bluetooth bulbs. Would you like '
                    'to add them?'
                : 'Rhythm found a nearby Hue Bluetooth bulb. Would you like '
                    'to add it?',
            style: const TextStyle(
              color: CelestialColors.textSecondary,
              height: 1.4,
            ),
          ),
          actions: [
            TextButton(
              key: const ValueKey('nearby-hue-ble-dismiss'),
              onPressed: () => Navigator.of(dialogContext).pop(false),
              child: const Text('Not Now'),
            ),
            FilledButton.icon(
              key: const ValueKey('nearby-hue-ble-accept'),
              onPressed: () => Navigator.of(dialogContext).pop(true),
              icon: const Icon(Icons.add_rounded),
              label: Text(discovery.deviceCount > 1 ? 'Add Bulbs' : 'Add Bulb'),
            ),
          ],
        ),
      ) ??
      false;

  await AnalyticsService().logHueBleNearbyPromptAnswered(
    source: source,
    outcome: accepted ? 'accepted' : 'dismissed',
  );
  return accepted;
}
