import 'package:flutter/material.dart';
import 'package:flutter/cupertino.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import '../onboarding/widgets/onboarding_orbit.dart';
import '../providers/home_provider.dart';

/// Schedule screen for viewing and editing sleep schedule.
class ScheduleScreen extends StatefulWidget {
  const ScheduleScreen({super.key});

  @override
  State<ScheduleScreen> createState() => _ScheduleScreenState();
}

class _ScheduleScreenState extends State<ScheduleScreen> {
  int _bedtimeHour = 22;
  int _bedtimeMinute = 30;
  int _wakeTimeHour = 6;
  int _wakeTimeMinute = 30;
  bool _isLoading = true;

  @override
  void initState() {
    super.initState();
    _loadSchedule();
  }

  Future<void> _loadSchedule() async {
    final homeProvider = context.read<HomeProvider>();
    final schedule = homeProvider.currentHome?.sleepSchedule;
    if (schedule != null) {
      // Convert from decimal hours (e.g., 22.5 = 10:30 PM) to hour/minute
      final bedtimeDecimal = schedule.bedtime;
      final wakeTimeDecimal = schedule.wakeTime;
      setState(() {
        _bedtimeHour = bedtimeDecimal.floor();
        _bedtimeMinute = ((bedtimeDecimal - _bedtimeHour) * 60).round();
        _wakeTimeHour = wakeTimeDecimal.floor();
        _wakeTimeMinute = ((wakeTimeDecimal - _wakeTimeHour) * 60).round();
        _isLoading = false;
      });
    } else {
      setState(() => _isLoading = false);
    }
  }

  Future<void> _saveSchedule() async {
    // Convert to decimal hours for SleepSchedule model
    final bedtime = _bedtimeHour + (_bedtimeMinute / 60.0);
    final wakeTime = _wakeTimeHour + (_wakeTimeMinute / 60.0);

    final schedule = SleepSchedule(
      bedtime: bedtime,
      wakeTime: wakeTime,
      enabled: true,
    );

    final homeProvider = context.read<HomeProvider>();
    await homeProvider.updateCurrentHomeSleepSchedule(schedule);
  }

  String _formatTime(int hour, int minute) {
    final use24h = MediaQuery.alwaysUse24HourFormatOf(context);
    if (use24h) {
      return '${hour.toString().padLeft(2, '0')}:${minute.toString().padLeft(2, '0')}';
    }
    final period = hour >= 12 ? 'PM' : 'AM';
    final displayHour = hour == 0 ? 12 : (hour > 12 ? hour - 12 : hour);
    return '${displayHour.toString()}:${minute.toString().padLeft(2, '0')} $period';
  }

  Future<void> _showTimePicker({
    required bool isBedtime,
    required int currentHour,
    required int currentMinute,
  }) async {
    await showCupertinoModalPopup<void>(
      context: context,
      builder: (BuildContext context) {
        int tempHour = currentHour;
        int tempMinute = currentMinute;

        return Container(
          height: 300,
          decoration: const BoxDecoration(
            color: OnboardingColors.backgroundCard,
            borderRadius: BorderRadius.vertical(top: Radius.circular(20)),
          ),
          child: Column(
            children: [
              // Header
              Container(
                padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
                decoration: BoxDecoration(
                  border: Border(
                    bottom: BorderSide(
                      color: OnboardingColors.orbitRing.withValues(alpha: 0.3),
                    ),
                  ),
                ),
                child: Row(
                  mainAxisAlignment: MainAxisAlignment.spaceBetween,
                  children: [
                    TextButton(
                      onPressed: () => Navigator.pop(context),
                      child: Text(
                        'Cancel',
                        style: TextStyle(
                          color: OnboardingColors.textSecondary,
                          fontSize: 16,
                        ),
                      ),
                    ),
                    Text(
                      isBedtime ? 'Bedtime' : 'Wake Time',
                      style: const TextStyle(
                        color: OnboardingColors.textPrimary,
                        fontSize: 17,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                    TextButton(
                      onPressed: () {
                        setState(() {
                          if (isBedtime) {
                            _bedtimeHour = tempHour;
                            _bedtimeMinute = tempMinute;
                          } else {
                            _wakeTimeHour = tempHour;
                            _wakeTimeMinute = tempMinute;
                          }
                        });
                        _saveSchedule();
                        Navigator.pop(context);
                      },
                      child: Text(
                        'Done',
                        style: TextStyle(
                          color: isBedtime
                              ? OnboardingColors.moonGlow
                              : OnboardingColors.sunWarm,
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
                        color: OnboardingColors.textPrimary,
                        fontSize: 22,
                      ),
                    ),
                  ),
                  child: CupertinoDatePicker(
                    mode: CupertinoDatePickerMode.time,
                    initialDateTime: DateTime(2024, 1, 1, currentHour, currentMinute),
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

  @override
  Widget build(BuildContext context) {
    if (_isLoading) {
      return Container(
        color: OnboardingColors.backgroundDark,
        child: const Center(
          child: CircularProgressIndicator(
            valueColor: AlwaysStoppedAnimation<Color>(OnboardingColors.sunWarm),
          ),
        ),
      );
    }

    return Container(
      color: OnboardingColors.backgroundDark,
      child: SafeArea(
        // Don't apply bottom safe area since we have the nav overlay
        bottom: false,
        child: SingleChildScrollView(
          padding: const EdgeInsets.symmetric(horizontal: 24),
          child: Column(
            children: [
              const SizedBox(height: 24),
              // Header
              const Text(
                'Sleep Schedule',
                style: TextStyle(
                  color: OnboardingColors.textPrimary,
                  fontSize: 28,
                  fontWeight: FontWeight.bold,
                  letterSpacing: 0.5,
                ),
              ),
              const SizedBox(height: 8),
              Text(
                'Tap to adjust your rhythm',
                style: TextStyle(
                  color: OnboardingColors.textSecondary,
                  fontSize: 15,
                ),
              ),
              const SizedBox(height: 32),
              // Orbit visualization
              OnboardingOrbit(
                size: 260,
                showTimeMarkers: true,
                bedtimeHour: _bedtimeHour,
                bedtimeMinute: _bedtimeMinute,
                wakeTimeHour: _wakeTimeHour,
                wakeTimeMinute: _wakeTimeMinute,
              ),
              const SizedBox(height: 40),
              // Time cards
              Row(
                children: [
                  // Bedtime card
                  Expanded(
                    child: _buildTimeCard(
                      icon: Icons.nightlight_round,
                      iconColor: OnboardingColors.moonGlow,
                      label: 'Bedtime',
                      time: _formatTime(_bedtimeHour, _bedtimeMinute),
                      onTap: () => _showTimePicker(
                        isBedtime: true,
                        currentHour: _bedtimeHour,
                        currentMinute: _bedtimeMinute,
                      ),
                    ),
                  ),
                  const SizedBox(width: 16),
                  // Wake time card
                  Expanded(
                    child: _buildTimeCard(
                      icon: Icons.wb_sunny_rounded,
                      iconColor: OnboardingColors.sunWarm,
                      label: 'Wake Time',
                      time: _formatTime(_wakeTimeHour, _wakeTimeMinute),
                      onTap: () => _showTimePicker(
                        isBedtime: false,
                        currentHour: _wakeTimeHour,
                        currentMinute: _wakeTimeMinute,
                      ),
                    ),
                  ),
                ],
              ),
              const SizedBox(height: 32),
              // Sleep duration info
              _buildSleepDurationCard(),
              // Bottom padding for nav overlay
              const SizedBox(height: 100),
            ],
          ),
        ),
      ),
    );
  }

  Widget _buildTimeCard({
    required IconData icon,
    required Color iconColor,
    required String label,
    required String time,
    required VoidCallback onTap,
  }) {
    return GestureDetector(
      onTap: onTap,
      child: Container(
        padding: const EdgeInsets.all(20),
        decoration: BoxDecoration(
          color: OnboardingColors.backgroundCard,
          borderRadius: BorderRadius.circular(20),
          border: Border.all(
            color: iconColor.withValues(alpha: 0.3),
            width: 1.5,
          ),
          boxShadow: [
            BoxShadow(
              color: iconColor.withValues(alpha: 0.1),
              blurRadius: 20,
              spreadRadius: 0,
            ),
          ],
        ),
        child: Column(
          children: [
            Container(
              width: 48,
              height: 48,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: iconColor.withValues(alpha: 0.15),
              ),
              child: Icon(
                icon,
                color: iconColor,
                size: 24,
              ),
            ),
            const SizedBox(height: 12),
            Text(
              label,
              style: TextStyle(
                color: OnboardingColors.textSecondary,
                fontSize: 13,
                fontWeight: FontWeight.w500,
              ),
            ),
            const SizedBox(height: 4),
            Text(
              time,
              style: const TextStyle(
                color: OnboardingColors.textPrimary,
                fontSize: 20,
                fontWeight: FontWeight.bold,
                letterSpacing: 0.5,
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildSleepDurationCard() {
    // Calculate sleep duration
    int bedtimeMinutes = _bedtimeHour * 60 + _bedtimeMinute;
    int wakeMinutes = _wakeTimeHour * 60 + _wakeTimeMinute;

    int durationMinutes = wakeMinutes - bedtimeMinutes;
    if (durationMinutes <= 0) {
      durationMinutes += 24 * 60; // Handle overnight sleep
    }

    final hours = durationMinutes ~/ 60;
    final minutes = durationMinutes % 60;

    String durationText = '${hours}h';
    if (minutes > 0) {
      durationText += ' ${minutes}m';
    }

    // Determine sleep quality indicator
    Color qualityColor;
    String qualityText;
    IconData qualityIcon;

    if (hours >= 7 && hours <= 9) {
      qualityColor = const Color(0xFF4CAF50);
      qualityText = 'Optimal';
      qualityIcon = Icons.check_circle;
    } else if (hours >= 6 && hours <= 10) {
      qualityColor = OnboardingColors.sunWarm;
      qualityText = 'Good';
      qualityIcon = Icons.info;
    } else {
      qualityColor = const Color(0xFFEF5350);
      qualityText = hours < 6 ? 'Too short' : 'Too long';
      qualityIcon = Icons.warning_rounded;
    }

    return Container(
      padding: const EdgeInsets.all(20),
      decoration: BoxDecoration(
        color: OnboardingColors.backgroundCard,
        borderRadius: BorderRadius.circular(16),
        border: Border.all(
          color: OnboardingColors.orbitRing.withValues(alpha: 0.5),
        ),
      ),
      child: Row(
        children: [
          Container(
            width: 56,
            height: 56,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: OnboardingColors.nightIndigo.withValues(alpha: 0.5),
            ),
            child: const Icon(
              Icons.bedtime_rounded,
              color: OnboardingColors.moonGlow,
              size: 28,
            ),
          ),
          const SizedBox(width: 16),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                const Text(
                  'Sleep Duration',
                  style: TextStyle(
                    color: OnboardingColors.textSecondary,
                    fontSize: 13,
                  ),
                ),
                const SizedBox(height: 2),
                Text(
                  durationText,
                  style: const TextStyle(
                    color: OnboardingColors.textPrimary,
                    fontSize: 24,
                    fontWeight: FontWeight.bold,
                  ),
                ),
              ],
            ),
          ),
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
            decoration: BoxDecoration(
              color: qualityColor.withValues(alpha: 0.15),
              borderRadius: BorderRadius.circular(20),
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Icon(
                  qualityIcon,
                  color: qualityColor,
                  size: 16,
                ),
                const SizedBox(width: 4),
                Text(
                  qualityText,
                  style: TextStyle(
                    color: qualityColor,
                    fontSize: 13,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}
