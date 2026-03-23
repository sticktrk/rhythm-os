import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../../../config/platform_capabilities.dart';
import '../../../widgets/settings_row.dart';
import '../../../providers/settings_provider.dart';
import '../../location_settings_screen.dart';
import '../preferences_screen.dart';
import '../light_tuning_screen.dart';

/// Preferences section for location, light settings, etc.
class PreferencesSection extends StatelessWidget {
  const PreferencesSection({super.key});

  @override
  Widget build(BuildContext context) {
    return Consumer<SettingsProvider>(
      builder: (context, settings, child) {
        final caps = context.read<PlatformCapabilities>();
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            const SettingsSectionHeader(title: 'Preferences'),
            SettingsGroup(
              children: [
                SettingsRow(
                  icon: Icons.tune_rounded,
                  iconColor: const Color(0xFFF9A825),
                  label: 'Preferences',
                  onTap: () => PreferencesScreen.show(context),
                ),
                SettingsRow(
                  icon: Icons.lightbulb_outline,
                  iconColor: const Color(0xFFA78BFA),
                  label: 'Light Tuning',
                  onTap: () => LightTuningScreen.show(context),
                ),
                if (caps.hasLocationSetup)
                  SettingsRow(
                    icon: Icons.location_on_outlined,
                    iconColor: const Color(0xFF4CAF50),
                    label: 'Location',
                    onTap: () => _showLocationSettings(context, settings),
                  ),
              ],
            ),
          ],
        );
      },
    );
  }

  Future<void> _showLocationSettings(BuildContext context, SettingsProvider settings) async {
    final result = await LocationSettingsScreen.show(context);
    if (result == true) {
      settings.loadSettings();
    }
  }
}
