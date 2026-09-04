import 'package:flutter/material.dart';
import 'info_tooltip.dart';
import 'solar_orbit.dart' show CelestialColors;

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

/// Shared room/bulb row for the node-level Low glow preference.
class LowGlowSettingRow extends StatelessWidget {
  const LowGlowSettingRow({
    super.key,
    required this.value,
    required this.onChanged,
  });

  final bool value;
  final ValueChanged<bool> onChanged;

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      behavior: HitTestBehavior.opaque,
      onTap: () => onChanged(!value),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 13),
        child: Row(
          children: [
            Icon(
              Icons.bedtime_outlined,
              color: CelestialColors.sunWarm.withValues(alpha: 0.8),
              size: 20,
            ),
            const SizedBox(width: 12),
            const Expanded(
              child: Row(
                children: [
                  Flexible(
                    child: Text(
                      'Low glow',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        color: CelestialColors.textPrimary,
                        fontSize: 15,
                      ),
                    ),
                  ),
                  SizedBox(width: 4),
                  InfoTooltip(
                    eyebrow: 'LOW GLOW',
                    accentColor: Color(0xFF7C83FF),
                    iconSize: 15,
                    message: 'Keep a soft low glow after motion times out or '
                        'you turn the light off. Motion or On restores normal '
                        'lighting.',
                  ),
                ],
              ),
            ),
            LowGlowSwitch(value: value, onChanged: onChanged),
          ],
        ),
      ),
    );
  }
}

/// Shared room/bulb entry point for per-node lighting overrides.
class LightingOverrideRow extends StatelessWidget {
  const LightingOverrideRow({
    super.key,
    required this.nodeId,
    required this.supported,
    required this.customized,
    required this.onPressed,
    this.settingsKeyPrefix = 'node-lighting',
    this.unsupportedStatus,
    this.unsupportedSemanticsValue,
    this.unsupportedIcon,
  });

  final String nodeId;
  final bool supported;
  final bool customized;
  final VoidCallback? onPressed;
  final String settingsKeyPrefix;
  final String? unsupportedStatus;
  final String? unsupportedSemanticsValue;
  final IconData? unsupportedIcon;

  @override
  Widget build(BuildContext context) {
    final interactive = onPressed != null;
    final status = !supported
        ? unsupportedStatus ?? 'Update required'
        : customized
            ? 'Custom'
            : 'Auto';
    final semanticsValue = !supported
        ? unsupportedSemanticsValue ?? 'Appliance update required'
        : customized
            ? 'Custom light settings'
            : 'Using automatic settings';
    final accent =
        customized ? const Color(0xFFF9A825) : CelestialColors.textSecondary;

    return Semantics(
      key: ValueKey('$settingsKeyPrefix-settings-$nodeId'),
      container: true,
      button: true,
      enabled: interactive,
      excludeSemantics: true,
      label: 'Lighting',
      value: semanticsValue,
      onTap: onPressed,
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: onPressed,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 13),
          child: Row(
            children: [
              Icon(
                supported
                    ? Icons.tune_rounded
                    : unsupportedIcon ?? Icons.system_update_rounded,
                color: CelestialColors.sunWarm.withValues(alpha: 0.8),
                size: 20,
              ),
              const SizedBox(width: 12),
              const Expanded(
                child: Text(
                  'Lighting',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 15,
                  ),
                ),
              ),
              Text(
                status,
                key: ValueKey('$settingsKeyPrefix-status-$nodeId'),
                style: TextStyle(
                  color: accent.withValues(
                    alpha: supported || customized ? 0.92 : 0.58,
                  ),
                  fontSize: 13,
                  fontWeight: customized ? FontWeight.w700 : FontWeight.w500,
                ),
              ),
              const SizedBox(width: 6),
              Icon(
                supported
                    ? Icons.chevron_right_rounded
                    : Icons.info_outline_rounded,
                size: 18,
                color: accent.withValues(alpha: supported ? 0.72 : 0.46),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// Compact cue for a node whose visible light curve differs from home.
class LightProfileOverrideBadge extends StatelessWidget {
  const LightProfileOverrideBadge({
    super.key,
    required this.nodeId,
    required this.brightnessRange,
    required this.colorTemperatureRange,
    required this.otherVisual,
  });

  final String nodeId;
  final bool brightnessRange;
  final bool colorTemperatureRange;
  final bool otherVisual;

  @override
  Widget build(BuildContext context) {
    if (!brightnessRange && !colorTemperatureRange && !otherVisual) {
      return const SizedBox.shrink();
    }

    final labels = <String>[
      if (brightnessRange) 'BRI',
      if (colorTemperatureRange) 'CCT',
      if (otherVisual) 'PROFILE',
    ];
    final descriptions = <String>[
      if (brightnessRange) 'brightness range',
      if (colorTemperatureRange) 'color temperature range',
      if (otherVisual) 'other light profile settings',
    ];
    final semanticsLabel =
        'Custom light profile: ${descriptions.join(' and ')}';

    return Tooltip(
      message: semanticsLabel,
      child: Semantics(
        key: ValueKey('light-profile-override-badge-$nodeId'),
        container: true,
        label: semanticsLabel,
        excludeSemantics: true,
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 3),
          decoration: BoxDecoration(
            color: const Color(0xE61C1813),
            borderRadius: BorderRadius.circular(99),
            border: Border.all(
              color: CelestialColors.warning.withValues(alpha: 0.7),
            ),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              const Icon(
                Icons.tune_rounded,
                size: 10,
                color: Color(0xFFFFCC80),
              ),
              const SizedBox(width: 3),
              Text(
                labels.join(' · '),
                style: const TextStyle(
                  color: Color(0xFFFFCC80),
                  fontSize: 8.5,
                  fontWeight: FontWeight.w800,
                  letterSpacing: 0.55,
                  height: 1,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
