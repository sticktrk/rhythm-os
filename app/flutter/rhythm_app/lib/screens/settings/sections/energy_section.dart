import 'package:flutter/material.dart';
import '../../../widgets/settings_row.dart';
import '../../power_usage_screen.dart';

/// Energy section with power usage estimation.
class EnergySection extends StatelessWidget {
  const EnergySection({super.key});

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const SettingsSectionHeader(title: 'Energy'),
        SettingsGroup(
          children: [
            SettingsRow(
              icon: Icons.bolt_rounded,
              iconColor: const Color(0xFF4ADE80),
              label: 'Power Usage',
              onTap: () => PowerUsageScreen.show(context),
            ),
          ],
        ),
      ],
    );
  }
}
