import 'package:flutter/cupertino.dart';
import 'package:flutter/material.dart';
import 'onboarding_orbit.dart';

/// Circular time picker styled with orbital ring aesthetic.
class TimeWheelPicker extends StatelessWidget {
  final String label;
  final IconData icon;
  final Color iconColor;
  final int hour;
  final int minute;
  final ValueChanged<(int, int)> onTimeChanged;

  const TimeWheelPicker({
    super.key,
    required this.label,
    required this.icon,
    required this.iconColor,
    required this.hour,
    required this.minute,
    required this.onTimeChanged,
  });

  String get _formattedTime {
    final h = hour % 12 == 0 ? 12 : hour % 12;
    final period = hour < 12 ? 'AM' : 'PM';
    return '${h.toString()}:${minute.toString().padLeft(2, '0')} $period';
  }

  void _showTimePicker(BuildContext context) {
    showModalBottomSheet(
      context: context,
      backgroundColor: Colors.transparent,
      builder: (context) => _TimePickerModal(
        initialHour: hour,
        initialMinute: minute,
        onTimeSelected: onTimeChanged,
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: () => _showTimePicker(context),
      child: Container(
        width: 140,
        height: 160,
        decoration: BoxDecoration(
          shape: BoxShape.circle,
          border: Border.all(
            color: OnboardingColors.orbitRing,
            width: 2,
          ),
          gradient: RadialGradient(
            colors: [
              OnboardingColors.backgroundCard,
              OnboardingColors.backgroundDark,
            ],
            stops: const [0.5, 1.0],
          ),
        ),
        child: Column(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Icon(
              icon,
              color: iconColor,
              size: 28,
            ),
            const SizedBox(height: 8),
            Text(
              label,
              style: TextStyle(
                color: OnboardingColors.textSecondary,
                fontSize: 12,
                fontWeight: FontWeight.w500,
                letterSpacing: 0.5,
              ),
            ),
            const SizedBox(height: 4),
            Text(
              _formattedTime,
              style: const TextStyle(
                color: OnboardingColors.textPrimary,
                fontSize: 18,
                fontWeight: FontWeight.w600,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// Modal bottom sheet time picker.
class _TimePickerModal extends StatefulWidget {
  final int initialHour;
  final int initialMinute;
  final ValueChanged<(int, int)> onTimeSelected;

  const _TimePickerModal({
    required this.initialHour,
    required this.initialMinute,
    required this.onTimeSelected,
  });

  @override
  State<_TimePickerModal> createState() => _TimePickerModalState();
}

class _TimePickerModalState extends State<_TimePickerModal> {
  late int _selectedHour;
  late int _selectedMinute;

  @override
  void initState() {
    super.initState();
    _selectedHour = widget.initialHour;
    _selectedMinute = widget.initialMinute;
  }

  @override
  Widget build(BuildContext context) {
    return Container(
      height: 340,
      decoration: const BoxDecoration(
        color: OnboardingColors.backgroundCard,
        borderRadius: BorderRadius.vertical(top: Radius.circular(24)),
      ),
      child: Column(
        children: [
          // Handle bar
          Container(
            margin: const EdgeInsets.symmetric(vertical: 12),
            width: 40,
            height: 4,
            decoration: BoxDecoration(
              color: OnboardingColors.orbitRing,
              borderRadius: BorderRadius.circular(2),
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
                initialDateTime: DateTime(
                  2024,
                  1,
                  1,
                  _selectedHour,
                  _selectedMinute,
                ),
                onDateTimeChanged: (dateTime) {
                  setState(() {
                    _selectedHour = dateTime.hour;
                    _selectedMinute = dateTime.minute;
                  });
                },
              ),
            ),
          ),
          // Confirm button
          Padding(
            padding: const EdgeInsets.fromLTRB(24, 8, 24, 24),
            child: SizedBox(
              width: double.infinity,
              child: ElevatedButton(
                onPressed: () {
                  widget.onTimeSelected((_selectedHour, _selectedMinute));
                  Navigator.of(context).pop();
                },
                style: ElevatedButton.styleFrom(
                  backgroundColor: OnboardingColors.sunWarm,
                  foregroundColor: OnboardingColors.backgroundDark,
                  padding: const EdgeInsets.symmetric(vertical: 16),
                  shape: RoundedRectangleBorder(
                    borderRadius: BorderRadius.circular(12),
                  ),
                ),
                child: const Text(
                  'Confirm',
                  style: TextStyle(
                    fontSize: 16,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}
