import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' show RhythmMode;
import 'package:uuid/uuid.dart';

import '../providers/server_sync_provider.dart';
import '../services/analytics_service.dart';
import 'celestial_segmented_control.dart';
import 'solar_orbit.dart' show CelestialColors;

// Menu chrome shared with the Alarm Schedule editor's elevated surfaces.
const Color _menuSurface = Color(0xFF1F2630);
const Color _menuHairline = Color(0xFF2C3441);

/// The four per-room behaviors available when Day or Night begins.
enum RoomScheduleBehavior { automatic, standby, off, on }

RoomScheduleBehavior roomScheduleBehaviorForState(String? state) =>
    switch (state) {
      'active' => RoomScheduleBehavior.on,
      'idle' || 'soft_off' || 'standby' => RoomScheduleBehavior.standby,
      'mood' || 'hard_off' => RoomScheduleBehavior.off,
      _ => RoomScheduleBehavior.automatic,
    };

String? stateForRoomScheduleBehavior(RoomScheduleBehavior behavior) =>
    switch (behavior) {
      RoomScheduleBehavior.on => 'active',
      RoomScheduleBehavior.standby => 'standby',
      RoomScheduleBehavior.off => 'hard_off',
      RoomScheduleBehavior.automatic => null,
    };

String roomScheduleBehaviorLabel(RoomScheduleBehavior behavior) =>
    switch (behavior) {
      RoomScheduleBehavior.automatic => 'Auto',
      RoomScheduleBehavior.standby => 'Low glow',
      RoomScheduleBehavior.off => 'Off',
      RoomScheduleBehavior.on => 'On',
    };

RoomScheduleBehavior nextRoomScheduleBehavior(RoomScheduleBehavior behavior) =>
    switch (behavior) {
      RoomScheduleBehavior.automatic => RoomScheduleBehavior.on,
      RoomScheduleBehavior.on => RoomScheduleBehavior.standby,
      RoomScheduleBehavior.standby => RoomScheduleBehavior.off,
      RoomScheduleBehavior.off => RoomScheduleBehavior.automatic,
    };

/// Shared commit path for both behavior pickers: optimistic provider write
/// plus the analytics event matching the surface that triggered it.
void _applyBehaviorSelection(
  BuildContext context, {
  required String roomId,
  required RhythmMode mode,
  required RoomScheduleBehavior selected,
  required String analyticsSource,
  required String inputMethod,
}) {
  context.read<ServerSyncProvider>().updateRoomDefaultForMode(
        roomId: roomId,
        mode: mode,
        state: stateForRoomScheduleBehavior(selected),
      );
  if (analyticsSource == 'room_schedule_tab') {
    AnalyticsService().logRoomScheduleInlinePresetChanged(
      journeyId: 'room-schedule-preset-${const Uuid().v4()}',
      attemptNumber: 1,
      inputMethod: inputMethod,
      mode: mode == RhythmMode.sleep ? 'sleep' : 'wake',
      behavior: selected.name,
    );
  } else {
    AnalyticsService().logLightProfileRoomDefaultChanged(
      profile: mode == RhythmMode.sleep ? 'sleep' : 'rhythm',
      cleared: selected == RoomScheduleBehavior.automatic,
      source: analyticsSource,
    );
  }
}

/// Compact access to a room's Day and Night automation behavior.
///
/// The Automations tab remains the full-room editor. This row intentionally
/// uses the same [ServerSyncProvider] state owner so both entry points stay in
/// sync and persist through the existing mode-config API.
class RoomScheduleBehaviorControl extends StatelessWidget {
  const RoomScheduleBehaviorControl({
    super.key,
    required this.roomId,
    required this.foregroundColor,
    this.enabled = true,
    this.showTopDivider = true,
    this.keyPrefix = 'room-card-schedule',
    this.analyticsSource = 'room_card',
  });

  final String roomId;
  final Color foregroundColor;
  final bool enabled;
  final bool showTopDivider;
  final String keyPrefix;
  final String analyticsSource;

  @override
  Widget build(BuildContext context) {
    return Selector<ServerSyncProvider, (String?, String?)>(
      selector: (_, sync) => (
        sync.roomDefaultStateForMode(roomId, RhythmMode.day),
        sync.roomDefaultStateForMode(roomId, RhythmMode.sleep),
      ),
      builder: (context, defaults, _) {
        return Container(
          key: ValueKey('$keyPrefix-$roomId'),
          padding: EdgeInsets.only(top: showTopDivider ? 8 : 0),
          decoration: showTopDivider
              ? BoxDecoration(
                  border: Border(
                    top: BorderSide(
                      color: foregroundColor.withValues(alpha: 0.10),
                    ),
                  ),
                )
              : null,
          child: Row(
            children: [
              Expanded(
                child: _ScheduleModeMenu(
                  roomId: roomId,
                  mode: RhythmMode.day,
                  title: 'Day',
                  icon: Icons.wb_sunny_rounded,
                  state: defaults.$1,
                  accent: const Color(0xFFF9A825),
                  foregroundColor: foregroundColor,
                  enabled: enabled,
                  keyPrefix: keyPrefix,
                  analyticsSource: analyticsSource,
                ),
              ),
              const SizedBox(width: 8),
              Expanded(
                child: _ScheduleModeMenu(
                  roomId: roomId,
                  mode: RhythmMode.sleep,
                  title: 'Night',
                  icon: Icons.bedtime_rounded,
                  state: defaults.$2,
                  accent: const Color(0xFF7C83FF),
                  foregroundColor: foregroundColor,
                  enabled: enabled,
                  keyPrefix: keyPrefix,
                  analyticsSource: analyticsSource,
                ),
              ),
            ],
          ),
        );
      },
    );
  }
}

class _ScheduleModeMenu extends StatelessWidget {
  const _ScheduleModeMenu({
    required this.roomId,
    required this.mode,
    required this.title,
    required this.icon,
    required this.state,
    required this.accent,
    required this.foregroundColor,
    required this.enabled,
    required this.keyPrefix,
    required this.analyticsSource,
  });

  final String roomId;
  final RhythmMode mode;
  final String title;
  final IconData icon;
  final String? state;
  final Color accent;
  final Color foregroundColor;
  final bool enabled;
  final String keyPrefix;
  final String analyticsSource;

  @override
  Widget build(BuildContext context) {
    final behavior = roomScheduleBehaviorForState(state);
    final label = roomScheduleBehaviorLabel(behavior);
    final modeKey = mode == RhythmMode.day ? 'day' : 'night';

    return Semantics(
      button: true,
      enabled: enabled,
      label: '$title schedule behavior',
      value: label,
      hint: 'Choose Auto, Low glow, Off, or On',
      excludeSemantics: true,
      child: PopupMenuButton<RoomScheduleBehavior>(
        key: ValueKey('$keyPrefix-$modeKey-$roomId'),
        enabled: enabled,
        initialValue: behavior,
        tooltip: '$title schedule behavior: $label',
        position: PopupMenuPosition.under,
        onSelected: (selected) {
          if (selected == behavior) return;
          _applyBehaviorSelection(
            context,
            roomId: roomId,
            mode: mode,
            selected: selected,
            analyticsSource: analyticsSource,
            inputMethod: 'popup_menu',
          );
        },
        color: _menuSurface,
        elevation: 12,
        shadowColor: Colors.black.withValues(alpha: 0.5),
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(14),
          side: const BorderSide(color: _menuHairline),
        ),
        constraints: const BoxConstraints(minWidth: 216, maxWidth: 248),
        itemBuilder: (_) => [
          for (final option in RoomScheduleBehavior.values)
            PopupMenuItem<RoomScheduleBehavior>(
              value: option,
              padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
              child: _BehaviorMenuRow(
                option: option,
                selected: option == behavior,
                accent: accent,
              ),
            ),
        ],
        child: Container(
          height: 44,
          padding: const EdgeInsets.symmetric(horizontal: 10),
          decoration: BoxDecoration(
            color: foregroundColor.withValues(alpha: 0.07),
            borderRadius: BorderRadius.circular(12),
            border: Border.all(
              color: foregroundColor.withValues(alpha: 0.10),
            ),
          ),
          child: Row(
            children: [
              Icon(icon, size: 15, color: accent.withValues(alpha: 0.90)),
              const SizedBox(width: 6),
              Text(
                title,
                style: TextStyle(
                  color: foregroundColor.withValues(alpha: 0.68),
                  fontSize: 11,
                  fontWeight: FontWeight.w600,
                ),
              ),
              const SizedBox(width: 8),
              Expanded(
                child: Text(
                  label,
                  textAlign: TextAlign.end,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: foregroundColor.withValues(alpha: 0.94),
                    fontSize: 12,
                    fontWeight: FontWeight.w700,
                  ),
                ),
              ),
              const SizedBox(width: 2),
              Icon(
                Icons.arrow_drop_down_rounded,
                size: 17,
                color: foregroundColor.withValues(alpha: 0.48),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// Full-width behavior picker for the room Schedule tab: a Wake row and a
/// Sleep row, each with all four behaviors visible as one 4-way segmented
/// control — one tap, nothing hidden behind a menu. Shares state ownership
/// and analytics with the compact popup variant used on room cards.
class RoomScheduleBehaviorSegments extends StatelessWidget {
  const RoomScheduleBehaviorSegments({
    super.key,
    required this.roomId,
    this.enabled = true,
    this.keyPrefix = 'room-schedule-presets',
    this.analyticsSource = 'room_schedule_tab',
  });

  final String roomId;
  final bool enabled;
  final String keyPrefix;
  final String analyticsSource;

  @override
  Widget build(BuildContext context) {
    return Selector<ServerSyncProvider, (String?, String?)>(
      selector: (_, sync) => (
        sync.roomDefaultStateForMode(roomId, RhythmMode.day),
        sync.roomDefaultStateForMode(roomId, RhythmMode.sleep),
      ),
      builder: (context, defaults, _) {
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            _BehaviorSegmentRow(
              key: ValueKey('$keyPrefix-day-$roomId'),
              roomId: roomId,
              mode: RhythmMode.day,
              title: 'Wake',
              icon: Icons.wb_sunny_rounded,
              accent: const Color(0xFFF9A825),
              state: defaults.$1,
              enabled: enabled,
              analyticsSource: analyticsSource,
            ),
            const SizedBox(height: 12),
            _BehaviorSegmentRow(
              key: ValueKey('$keyPrefix-night-$roomId'),
              roomId: roomId,
              mode: RhythmMode.sleep,
              title: 'Sleep',
              icon: Icons.bedtime_rounded,
              accent: const Color(0xFF7C83FF),
              state: defaults.$2,
              enabled: enabled,
              analyticsSource: analyticsSource,
            ),
          ],
        );
      },
    );
  }
}

class _BehaviorSegmentRow extends StatelessWidget {
  const _BehaviorSegmentRow({
    super.key,
    required this.roomId,
    required this.mode,
    required this.title,
    required this.icon,
    required this.accent,
    required this.state,
    required this.enabled,
    required this.analyticsSource,
  });

  final String roomId;
  final RhythmMode mode;
  final String title;
  final IconData icon;
  final Color accent;
  final String? state;
  final bool enabled;
  final String analyticsSource;

  @override
  Widget build(BuildContext context) {
    final behavior = roomScheduleBehaviorForState(state);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Opacity(
          opacity: enabled ? 1.0 : 0.5,
          child: Padding(
            padding: const EdgeInsets.only(left: 4),
            child: Row(
              children: [
                Icon(icon, size: 13, color: accent),
                const SizedBox(width: 6),
                Text(
                  title.toUpperCase(),
                  style: TextStyle(
                    color: accent,
                    fontSize: 11,
                    fontWeight: FontWeight.w700,
                    letterSpacing: 0.6,
                  ),
                ),
              ],
            ),
          ),
        ),
        const SizedBox(height: 6),
        CelestialSegmentedControl<RoomScheduleBehavior>(
          segments: [
            // Ordered as a brightness scale rather than enum order.
            for (final option in const [
              RoomScheduleBehavior.automatic,
              RoomScheduleBehavior.on,
              RoomScheduleBehavior.standby,
              RoomScheduleBehavior.off,
            ])
              CelestialSegment(
                value: option,
                label: roomScheduleBehaviorLabel(option),
                accent: accent,
                semanticLabel: '$title ${roomScheduleBehaviorLabel(option)}',
              ),
          ],
          selected: behavior,
          onTap: (option) => _applyBehaviorSelection(
            context,
            roomId: roomId,
            mode: mode,
            selected: option,
            analyticsSource: analyticsSource,
            inputMethod: 'segment',
          ),
          enabled: enabled,
          density: CelestialSegmentedDensity.compact,
        ),
      ],
    );
  }
}

/// One row of the behavior menu: tinted icon disc, label + one-line meaning,
/// accent check on the current choice. Styled to the celestial palette rather
/// than the stock Material menu row.
class _BehaviorMenuRow extends StatelessWidget {
  const _BehaviorMenuRow({
    required this.option,
    required this.selected,
    required this.accent,
  });

  final RoomScheduleBehavior option;
  final bool selected;
  final Color accent;

  (IconData, Color, String) get _visual => switch (option) {
        RoomScheduleBehavior.automatic => (
            Icons.auto_awesome_rounded,
            CelestialColors.accentBlue,
            'Follows the lighting curve',
          ),
        RoomScheduleBehavior.on => (
            Icons.lightbulb_rounded,
            const Color(0xFF3FB950),
            'Lights turn on',
          ),
        RoomScheduleBehavior.standby => (
            Icons.nightlight_round,
            const Color(0xFFFFB74D),
            'Faint night-light glow',
          ),
        RoomScheduleBehavior.off => (
            Icons.power_settings_new_rounded,
            CelestialColors.textSecondary,
            'Lights turn off',
          ),
      };

  @override
  Widget build(BuildContext context) {
    final (icon, iconColor, meaning) = _visual;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 7),
      decoration: selected
          ? BoxDecoration(
              color: accent.withValues(alpha: 0.10),
              borderRadius: BorderRadius.circular(10),
              border: Border.all(color: accent.withValues(alpha: 0.28)),
            )
          : null,
      child: Row(
        children: [
          Container(
            width: 28,
            height: 28,
            alignment: Alignment.center,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: iconColor.withValues(alpha: 0.14),
              border: Border.all(color: iconColor.withValues(alpha: 0.32)),
            ),
            child: Icon(icon, size: 14, color: iconColor),
          ),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(
                  roomScheduleBehaviorLabel(option),
                  style: TextStyle(
                    color: CelestialColors.textPrimary
                        .withValues(alpha: selected ? 1.0 : 0.92),
                    fontSize: 13.5,
                    fontWeight: FontWeight.w600,
                    letterSpacing: 0.1,
                  ),
                ),
                const SizedBox(height: 1),
                Text(
                  meaning,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: CelestialColors.textSecondary
                        .withValues(alpha: selected ? 0.85 : 0.65),
                    fontSize: 10.5,
                    height: 1.2,
                  ),
                ),
              ],
            ),
          ),
          if (selected) ...[
            const SizedBox(width: 6),
            Icon(Icons.check_rounded, size: 16, color: accent),
          ],
        ],
      ),
    );
  }
}
