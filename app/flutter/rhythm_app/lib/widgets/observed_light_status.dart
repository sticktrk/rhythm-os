import 'package:flutter/material.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

/// Reports physical feedback without changing the engine's desired profiles.
class ObservedLightStatus extends StatelessWidget {
  const ObservedLightStatus({super.key, required this.observation});

  final RhythmObservedLight observation;

  @override
  Widget build(BuildContext context) {
    final status = switch (observation.availability) {
      RhythmLightAvailability.unavailable => 'Unavailable in Home Assistant',
      RhythmLightAvailability.unknown => 'State unknown in Home Assistant',
      RhythmLightAvailability.disconnected => 'Waiting for Home Assistant',
      RhythmLightAvailability.available => switch (
            observation.currentLightsOn) {
          true => 'On',
          false => 'Off',
          null => 'Power state unknown',
        },
    };
    final detail = <String>[
      status,
      if (observation.isAvailable && observation.currentLightsOn == true) ...[
        if (observation.brightness != null) '${observation.brightness}%',
        if (observation.kelvin != null) '${observation.kelvin} K',
        if (observation.rgb != null)
          'RGB ${observation.rgb!.$1}, ${observation.rgb!.$2}, ${observation.rgb!.$3}',
        if (observation.rgb == null && observation.xy != null) 'Color light',
      ],
    ].join(' · ');
    return ListTile(
      key: const ValueKey('observed-light-status'),
      contentPadding: EdgeInsets.zero,
      leading: Icon(observation.isAvailable
          ? Icons.lightbulb_outline
          : Icons.cloud_off_outlined),
      title: const Text('Observed light'),
      subtitle: Text(detail),
    );
  }
}
