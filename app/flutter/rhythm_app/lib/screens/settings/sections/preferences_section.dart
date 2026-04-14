import 'package:flutter/material.dart';
import '../../../widgets/settings_row.dart';
import '../light_profile_screen.dart';

/// Light Profile section with Day and Sleep profile buttons.
class PreferencesSection extends StatelessWidget {
  const PreferencesSection({super.key});

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const SettingsSectionHeader(title: 'Light Profile'),
        SettingsGroup(
          children: [
            SettingsRow(
              icon: Icons.wb_sunny_rounded,
              iconColor: const Color(0xFFF9A825),
              label: 'Day Profile',
              onTap: () =>
                  LightProfileScreen.show(context, initialProfile: 'rhythm'),
            ),
            SettingsRow(
              icon: Icons.bedtime_rounded,
              iconColor: const Color(0xFF7C4DFF),
              label: 'Sleep Profile',
              onTap: () =>
                  LightProfileScreen.show(context, initialProfile: 'sleep'),
            ),
          ],
        ),
      ],
    );
  }
}
