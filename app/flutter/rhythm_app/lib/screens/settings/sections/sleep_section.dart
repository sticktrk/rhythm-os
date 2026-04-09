import 'package:flutter/material.dart';
import 'package:flutter/cupertino.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import '../../../widgets/solar_orbit.dart'; // For CelestialColors
import '../../../widgets/settings_row.dart';
import '../../../onboarding/widgets/onboarding_orbit.dart';
import '../../../providers/settings_provider.dart';
import '../../../providers/home_provider.dart';

/// Sleep schedule section with bedtime/wake time pickers.
class SleepSection extends StatelessWidget {
  const SleepSection({super.key});

  @override
  Widget build(BuildContext context) {
    return Consumer<SettingsProvider>(
      builder: (context, settings, child) {
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            const SettingsSectionHeader(title: 'Sleep Schedule'),
            _buildSleepScheduleCard(context, settings),
          ],
        );
      },
    );
  }

  Widget _buildSleepScheduleCard(
      BuildContext context, SettingsProvider settings) {
    return Container(
      margin: const EdgeInsets.only(bottom: 8),
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(14),
      ),
      child: Column(
        children: [
          // Wake Time and Bedtime row
          Row(
            children: [
              // Wake Time
              Expanded(
                child: _buildTimeButton(
                  context: context,
                  settings: settings,
                  icon: Icons.wb_sunny_rounded,
                  iconColor: const Color(0xFFFF9800),
                  label: 'Wake Time',
                  time: settings.formatTime(
                      settings.wakeTimeHour, settings.wakeTimeMinute,
                      use24h: MediaQuery.alwaysUse24HourFormatOf(context)),
                  onTap: () => _showTimePicker(
                    context: context,
                    settings: settings,
                    isBedtime: false,
                    currentHour: settings.wakeTimeHour,
                    currentMinute: settings.wakeTimeMinute,
                  ),
                ),
              ),
              // Divider
              Container(
                width: 1,
                height: 50,
                margin: const EdgeInsets.symmetric(horizontal: 12),
                color: CelestialColors.orbitRing.withValues(alpha: 0.3),
              ),
              // Bedtime
              Expanded(
                child: _buildTimeButton(
                  context: context,
                  settings: settings,
                  icon: Icons.nightlight_round,
                  iconColor: OnboardingColors.moonGlow,
                  label: 'Bedtime',
                  time: settings.formatTime(
                      settings.bedtimeHour, settings.bedtimeMinute,
                      use24h: MediaQuery.alwaysUse24HourFormatOf(context)),
                  onTap: () => _showTimePicker(
                    context: context,
                    settings: settings,
                    isBedtime: true,
                    currentHour: settings.bedtimeHour,
                    currentMinute: settings.bedtimeMinute,
                  ),
                ),
              ),
            ],
          ),
          const SizedBox(height: 12),
          // Sleep duration
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
            decoration: BoxDecoration(
              color: OnboardingColors.nightIndigo.withValues(alpha: 0.3),
              borderRadius: BorderRadius.circular(10),
            ),
            child: Row(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                Icon(
                  Icons.bedtime_rounded,
                  color: OnboardingColors.moonGlow.withValues(alpha: 0.8),
                  size: 16,
                ),
                const SizedBox(width: 8),
                Text(
                  'Sleep: ${settings.getSleepDuration()}',
                  style: TextStyle(
                    color: CelestialColors.textSecondary,
                    fontSize: 13,
                    fontWeight: FontWeight.w500,
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildTimeButton({
    required BuildContext context,
    required SettingsProvider settings,
    required IconData icon,
    required Color iconColor,
    required String label,
    required String time,
    required VoidCallback onTap,
  }) {
    return GestureDetector(
      onTap: onTap,
      behavior: HitTestBehavior.opaque,
      child: Row(
        children: [
          Container(
            width: 36,
            height: 36,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: iconColor,
            ),
            child: Icon(
              icon,
              color: const Color(0xFF1A1A1A),
              size: 18,
            ),
          ),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  label,
                  style: TextStyle(
                    color: CelestialColors.textSecondary,
                    fontSize: 11,
                    fontWeight: FontWeight.w500,
                  ),
                ),
                const SizedBox(height: 2),
                Text(
                  time,
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 16,
                    fontWeight: FontWeight.w600,
                    letterSpacing: 0.3,
                  ),
                ),
              ],
            ),
          ),
          Icon(
            Icons.chevron_right,
            color: CelestialColors.textSecondary.withValues(alpha: 0.5),
            size: 18,
          ),
        ],
      ),
    );
  }

  Future<void> _showTimePicker({
    required BuildContext context,
    required SettingsProvider settings,
    required bool isBedtime,
    required int currentHour,
    required int currentMinute,
  }) async {
    await showCupertinoModalPopup<void>(
      context: context,
      builder: (BuildContext popupContext) {
        int tempHour = currentHour;
        int tempMinute = currentMinute;

        return Container(
          height: 300,
          decoration: const BoxDecoration(
            color: CelestialColors.backgroundCard,
            borderRadius: BorderRadius.vertical(top: Radius.circular(20)),
          ),
          child: Column(
            children: [
              // Header
              Container(
                padding:
                    const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
                decoration: BoxDecoration(
                  border: Border(
                    bottom: BorderSide(
                      color: CelestialColors.orbitRing.withValues(alpha: 0.3),
                    ),
                  ),
                ),
                child: Row(
                  mainAxisAlignment: MainAxisAlignment.spaceBetween,
                  children: [
                    TextButton(
                      onPressed: () => Navigator.pop(popupContext),
                      child: const Text(
                        'Cancel',
                        style: TextStyle(
                          color: CelestialColors.textSecondary,
                          fontSize: 16,
                        ),
                      ),
                    ),
                    Text(
                      isBedtime ? 'Bedtime' : 'Wake Time',
                      style: const TextStyle(
                        color: CelestialColors.textPrimary,
                        fontSize: 17,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                    TextButton(
                      onPressed: () async {
                        // Update sleep schedule via HomeProvider
                        final homeProvider = context.read<HomeProvider>();
                        final currentSchedule =
                            homeProvider.currentHome?.sleepSchedule ??
                                SleepSchedule.defaults();

                        SleepSchedule newSchedule;
                        if (isBedtime) {
                          final bedtime = tempHour + (tempMinute / 60.0);
                          newSchedule =
                              currentSchedule.copyWith(bedtime: bedtime);
                        } else {
                          final wakeTime = tempHour + (tempMinute / 60.0);
                          newSchedule =
                              currentSchedule.copyWith(wakeTime: wakeTime);
                        }

                        await homeProvider
                            .updateCurrentHomeSleepSchedule(newSchedule);
                        if (!popupContext.mounted) return;
                        Navigator.pop(popupContext);
                      },
                      child: Text(
                        'Done',
                        style: TextStyle(
                          color: isBedtime
                              ? OnboardingColors.moonGlow
                              : CelestialColors.sunWarm,
                          fontSize: 16,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
              // Time picker
              Expanded(
                child: CupertinoTheme(
                  data: const CupertinoThemeData(
                    brightness: Brightness.dark,
                    textTheme: CupertinoTextThemeData(
                      dateTimePickerTextStyle: TextStyle(
                        color: CelestialColors.textPrimary,
                        fontSize: 22,
                      ),
                    ),
                  ),
                  child: CupertinoDatePicker(
                    mode: CupertinoDatePickerMode.time,
                    initialDateTime:
                        DateTime(2024, 1, 1, currentHour, currentMinute),
                    use24hFormat: MediaQuery.alwaysUse24HourFormatOf(context),
                    onDateTimeChanged: (DateTime newTime) {
                      tempHour = newTime.hour;
                      tempMinute = newTime.minute;
                    },
                  ),
                ),
              ),
            ],
          ),
        );
      },
    );
  }
}
