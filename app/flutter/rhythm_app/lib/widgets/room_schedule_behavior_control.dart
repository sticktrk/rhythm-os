import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' show RhythmMode;

import '../providers/server_sync_provider.dart';
import '../services/analytics_service.dart';

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
      RoomScheduleBehavior.standby => 'Standby',
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
      hint: 'Choose Auto, Standby, Off, or On',
      excludeSemantics: true,
      child: PopupMenuButton<RoomScheduleBehavior>(
        key: ValueKey('$keyPrefix-$modeKey-$roomId'),
        enabled: enabled,
        initialValue: behavior,
        tooltip: '$title schedule behavior: $label',
        position: PopupMenuPosition.under,
        onSelected: (selected) {
          if (selected == behavior) return;
          context.read<ServerSyncProvider>().updateRoomDefaultForMode(
                roomId: roomId,
                mode: mode,
                state: stateForRoomScheduleBehavior(selected),
              );
          AnalyticsService().logLightProfileRoomDefaultChanged(
            profile: mode == RhythmMode.sleep ? 'sleep' : 'rhythm',
            cleared: selected == RoomScheduleBehavior.automatic,
            source: analyticsSource,
          );
        },
        itemBuilder: (_) => [
          for (final option in RoomScheduleBehavior.values)
            PopupMenuItem<RoomScheduleBehavior>(
              value: option,
              child: Row(
                children: [
                  SizedBox(
                    width: 22,
                    child: option == behavior
                        ? Icon(Icons.check_rounded, size: 18, color: accent)
                        : null,
                  ),
                  const SizedBox(width: 8),
                  Text(roomScheduleBehaviorLabel(option)),
                ],
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
