import 'package:flutter/material.dart';

/// User-facing toggle for the existing Standby preference.
///
/// "Low glow" is presentation language only. The server continues to own and
/// persist the `standby_enabled` contract.
class LowGlowSwitch extends StatelessWidget {
  final bool value;
  final ValueChanged<bool> onChanged;

  const LowGlowSwitch({
    super.key,
    required this.value,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    return Semantics(
      excludeSemantics: true,
      label: 'Keep a low glow when inactive',
      value: value ? 'On' : 'Off',
      toggled: value,
      onTap: () => onChanged(!value),
      child: Switch.adaptive(
        value: value,
        onChanged: onChanged,
        activeTrackColor: const Color(0xFF7C83FF),
      ),
    );
  }
}
