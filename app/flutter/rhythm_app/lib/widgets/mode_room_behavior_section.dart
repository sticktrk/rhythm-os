import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart' hide Home, Hub, HubType;
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../providers/room_provider.dart';
import '../providers/server_sync_provider.dart';
import '../services/analytics_service.dart';
import '../utils/room_visibility.dart';

/// Per-room On / Standby / Off controls for a single [RhythmMode].
///
/// This is the "what each room does when Day/Sleep engages" behavior — lifted
/// out of the light-profile screen so the Automations tab can present it as its
/// own automation (the light *look* lives on the Light tab; turning lights
/// on/off/standby is an automation). Reads the room defaults from
/// [ServerSyncProvider.modeConfigs] and debounces writes back through
/// `api.modeSet`, exactly as the profile screen used to.
class ModeRoomBehaviorSection extends StatefulWidget {
  /// Which mode's room behavior to edit — Day or Sleep.
  final RhythmMode mode;

  const ModeRoomBehaviorSection({super.key, required this.mode});

  @override
  State<ModeRoomBehaviorSection> createState() =>
      _ModeRoomBehaviorSectionState();
}

class _ModeRoomBehaviorSectionState extends State<ModeRoomBehaviorSection> {
  Timer? _roomDefaultsDebounce;

  // Optimistic local copy, held only once the user has made an edit, so rapid
  // toggles feel instant despite the 800ms write debounce. Null until the first
  // edit — until then we reflect the live provider state directly (which also
  // lets us pick up mode configs that arrive after the first frame).
  List<RhythmModeConfig>? _localConfigs;

  @override
  void dispose() {
    _roomDefaultsDebounce?.cancel();
    super.dispose();
  }

  List<RhythmModeConfig> _effectiveConfigs(ServerSyncProvider sync) =>
      _localConfigs ?? sync.modeConfigs;

  Map<String, String> _roomDefaults(ServerSyncProvider sync) {
    for (final config in _effectiveConfigs(sync)) {
      if (config.mode == widget.mode) {
        return {for (final rd in config.roomDefaults) rd.roomId: rd.state};
      }
    }
    return const {};
  }

  void _onRoomDefaultChanged(String roomId, String? newState) {
    final sync = context.read<ServerSyncProvider>();
    final defaults = Map<String, String>.from(_roomDefaults(sync));
    if (newState == null) {
      defaults.remove(roomId);
    } else {
      defaults[roomId] = newState;
    }

    final updatedRoomDefaults = defaults.entries
        .map((e) => RoomDefault(roomId: e.key, state: e.value))
        .toList();

    final base = _effectiveConfigs(sync);
    final hasConfig = base.any((c) => c.mode == widget.mode);
    final List<RhythmModeConfig> updatedConfigs;
    if (hasConfig) {
      updatedConfigs = base.map((config) {
        if (config.mode == widget.mode) {
          return config.copyWith(roomDefaults: updatedRoomDefaults);
        }
        return config;
      }).toList();
    } else {
      updatedConfigs = [
        ...base,
        RhythmModeConfig(
          mode: widget.mode,
          activeProfileId: '',
          roomDefaults: updatedRoomDefaults,
        ),
      ];
    }

    // Update UI immediately, debounce the server push.
    setState(() => _localConfigs = updatedConfigs);

    _roomDefaultsDebounce?.cancel();
    _roomDefaultsDebounce = Timer(const Duration(milliseconds: 800), () {
      if (!mounted) return;
      context.read<ServerSyncProvider>().api.modeSet(configs: _localConfigs!);
    });
    AnalyticsService().logLightProfileRoomDefaultChanged(
      profile: widget.mode == RhythmMode.sleep ? 'sleep' : 'rhythm',
      cleared: newState == null,
    );
  }

  @override
  Widget build(BuildContext context) {
    return Consumer<ServerSyncProvider>(
      builder: (context, sync, _) {
        final defaults = _roomDefaults(sync);
        return Selector<RoomProvider, List<RoomDto>>(
          selector: (_, provider) =>
              provider.rooms.where(showsInAllRooms).toList(growable: false),
          builder: (context, rooms, __) {
            if (rooms.isEmpty) {
              return Padding(
                padding: const EdgeInsets.symmetric(vertical: 8),
                child: Text(
                  'No rooms yet.',
                  style: TextStyle(
                    color: _Palette.textSecondary.withValues(alpha: 0.7),
                    fontSize: 13,
                    fontWeight: FontWeight.w500,
                  ),
                ),
              );
            }
            return Column(
              children: [
                for (int i = 0; i < rooms.length; i++) ...[
                  if (i > 0) const SizedBox(height: 6),
                  _RoomDefaultCard(
                    key: ValueKey(rooms[i].id),
                    roomId: rooms[i].id,
                    roomName: rooms[i].name,
                    state: defaults[rooms[i].id],
                    onStateChanged: (newState) =>
                        _onRoomDefaultChanged(rooms[i].id, newState),
                  ),
                ],
              ],
            );
          },
        );
      },
    );
  }
}

// ---------------------------------------------------------------------------
// Room default state model + helpers
// ---------------------------------------------------------------------------

enum _RoomDefaultMode { none, off, standby, active }

_RoomDefaultMode _roomDefaultModeForState(String? state) => switch (state) {
      'active' => _RoomDefaultMode.active,
      'idle' || 'soft_off' || 'standby' => _RoomDefaultMode.standby,
      'mood' || 'hard_off' => _RoomDefaultMode.off,
      _ => _RoomDefaultMode.none,
    };

String? _stateFromRoomDefaultMode(_RoomDefaultMode mode) => switch (mode) {
      _RoomDefaultMode.active => 'active',
      _RoomDefaultMode.standby => 'standby',
      _RoomDefaultMode.off => 'hard_off',
      _RoomDefaultMode.none => null,
    };

_RoomDefaultMode _nextRoomDefaultMode(_RoomDefaultMode mode) => switch (mode) {
      _RoomDefaultMode.active => _RoomDefaultMode.standby,
      _RoomDefaultMode.standby => _RoomDefaultMode.off,
      _RoomDefaultMode.off => _RoomDefaultMode.none,
      _RoomDefaultMode.none => _RoomDefaultMode.active,
    };

String _roomDefaultLabel(_RoomDefaultMode mode) => switch (mode) {
      _RoomDefaultMode.active => 'On',
      _RoomDefaultMode.standby => 'Standby',
      _RoomDefaultMode.off => 'Off',
      _RoomDefaultMode.none => 'No override',
    };

@visibleForTesting
String roomDefaultStateLabelForTesting(String? state) =>
    _roomDefaultLabel(_roomDefaultModeForState(state));

@visibleForTesting
String? nextRoomDefaultStateForTesting(String? state) =>
    _stateFromRoomDefaultMode(
      _nextRoomDefaultMode(_roomDefaultModeForState(state)),
    );

class _RoomDefaultCard extends StatelessWidget {
  final String roomId;
  final String roomName;
  final String? state; // null = no override, "active", "standby", "hard_off"
  final ValueChanged<String?> onStateChanged;

  const _RoomDefaultCard({
    super.key,
    required this.roomId,
    required this.roomName,
    required this.state,
    required this.onStateChanged,
  });

  _RoomDefaultMode get _mode => _roomDefaultModeForState(state);

  @override
  Widget build(BuildContext context) {
    final mode = _mode;
    final hasOverride = mode != _RoomDefaultMode.none;

    final bgColor = switch (mode) {
      _RoomDefaultMode.active => const Color(0xFF1E1A12),
      _RoomDefaultMode.standby => const Color(0xFF1D1A13),
      _RoomDefaultMode.off || _RoomDefaultMode.none => _Palette.card,
    };

    final stateLabel = _roomDefaultLabel(mode);

    final stateLabelColor = switch (mode) {
      _RoomDefaultMode.active => const Color(0xFFD4A020),
      _RoomDefaultMode.standby => _Palette.idle,
      _RoomDefaultMode.off => _Palette.textSecondary,
      _RoomDefaultMode.none => _Palette.textSecondary.withValues(alpha: 0.4),
    };

    final indicatorColor = switch (mode) {
      _RoomDefaultMode.active => const Color(0xFFD4A020),
      _RoomDefaultMode.standby => _Palette.idle.withValues(alpha: 0.9),
      _RoomDefaultMode.off => _Palette.textSecondary.withValues(alpha: 0.8),
      _RoomDefaultMode.none => _Palette.textSecondary.withValues(alpha: 0.75),
    };

    return Selector<RoomProvider,
        ({MotionTimerInfo? motionTimer, bool hasSensor})>(
      selector: (_, provider) => (
        motionTimer: provider.getMotionTimer(roomId),
        hasSensor: provider.hasMotionSensor(roomId),
      ),
      builder: (context, motionState, child) {
        final motionTimer = motionState.motionTimer;
        final hasSensor = motionState.hasSensor;

        return GestureDetector(
          onLongPress: () {
            HapticFeedback.lightImpact();
            onStateChanged('hard_off');
          },
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 300),
            curve: Curves.easeInOut,
            decoration: BoxDecoration(
              color: bgColor,
              borderRadius: BorderRadius.circular(12),
              border: Border.all(
                color: hasOverride
                    ? _Palette.border
                    : _Palette.border.withValues(alpha: 0.4),
              ),
            ),
            padding: const EdgeInsets.fromLTRB(12, 10, 10, 10),
            child: Row(
              children: [
                if (motionTimer != null)
                  Padding(
                    padding: const EdgeInsets.only(right: 10),
                    child: _RoomDefaultMotionIndicator(
                      info: motionTimer,
                      color: indicatorColor,
                      onExpired: () =>
                          context.read<RoomProvider>().clearMotionTimer(roomId),
                    ),
                  )
                else if (hasSensor)
                  Padding(
                    padding: const EdgeInsets.only(right: 10),
                    child: Icon(
                      Icons.sensors_rounded,
                      size: 16,
                      color: indicatorColor.withValues(alpha: 0.45),
                    ),
                  ),
                Expanded(
                  child: AnimatedOpacity(
                    opacity: hasOverride ? 1.0 : 0.55,
                    duration: const Duration(milliseconds: 300),
                    child: Text(
                      roomName,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(
                        color: _Palette.textPrimary,
                        fontSize: 14,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                  ),
                ),
                if (hasOverride) ...[
                  const SizedBox(width: 8),
                  Text(
                    stateLabel,
                    style: TextStyle(
                      color: stateLabelColor,
                      fontSize: 11,
                      fontWeight: FontWeight.w500,
                    ),
                  ),
                ],
                const SizedBox(width: 10),
                _DefaultStateToggle(
                  mode: mode,
                  onModeChanged: (newMode) {
                    HapticFeedback.lightImpact();
                    onStateChanged(_stateFromRoomDefaultMode(newMode));
                  },
                ),
              ],
            ),
          ),
        );
      },
    );
  }
}

class _RoomDefaultMotionIndicator extends StatefulWidget {
  final MotionTimerInfo info;
  final Color color;
  final VoidCallback? onExpired;

  const _RoomDefaultMotionIndicator({
    required this.info,
    required this.color,
    this.onExpired,
  });

  @override
  State<_RoomDefaultMotionIndicator> createState() =>
      _RoomDefaultMotionIndicatorState();
}

class _RoomDefaultMotionIndicatorState
    extends State<_RoomDefaultMotionIndicator>
    with SingleTickerProviderStateMixin {
  Timer? _countdownTimer;
  int _interpolatedRemaining = 0;
  late final AnimationController _pulseController;

  @override
  void initState() {
    super.initState();
    _pulseController = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 1500),
    );
    _syncFromInfo();
  }

  @override
  void didUpdateWidget(_RoomDefaultMotionIndicator oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.info != widget.info) {
      _syncFromInfo();
    }
  }

  void _syncFromInfo() {
    if (widget.info.motionActive) {
      _countdownTimer?.cancel();
      _countdownTimer = null;
      if (!_pulseController.isAnimating) {
        _pulseController.repeat(reverse: true);
      }
      return;
    }

    _pulseController.stop();
    _pulseController.value = 0;

    if (widget.info.remainingSecs != null) {
      _interpolatedRemaining = widget.info.remainingSecs!;
      _startCountdown();
      return;
    }

    _countdownTimer?.cancel();
    _countdownTimer = null;
  }

  void _startCountdown() {
    _countdownTimer?.cancel();
    _countdownTimer = Timer.periodic(const Duration(seconds: 1), (_) {
      if (!mounted) return;
      final elapsed =
          DateTime.now().difference(widget.info.receivedAt).inSeconds;
      final remaining = (widget.info.remainingSecs ?? 0) - elapsed;
      if (remaining <= 0) {
        _countdownTimer?.cancel();
        _countdownTimer = null;
        widget.onExpired?.call();
        return;
      }
      setState(() {
        _interpolatedRemaining = remaining.clamp(0, widget.info.timeoutSecs);
      });
    });
  }

  @override
  void dispose() {
    _countdownTimer?.cancel();
    _pulseController.dispose();
    super.dispose();
  }

  String _formatTime(int secs) {
    if (secs >= 60) return '${(secs / 60).ceil()}m';
    return '${secs}s';
  }

  @override
  Widget build(BuildContext context) {
    if (widget.info.motionActive) {
      return AnimatedBuilder(
        animation: _pulseController,
        builder: (context, child) {
          final scale = 1.0 + _pulseController.value * 0.1;
          return Transform.scale(scale: scale, child: child);
        },
        child: Icon(
          Icons.directions_walk_rounded,
          size: 20,
          color: widget.color,
        ),
      );
    }

    final progress = widget.info.timeoutSecs > 0
        ? _interpolatedRemaining / widget.info.timeoutSecs
        : 0.0;

    return SizedBox(
      width: 28,
      height: 28,
      child: CustomPaint(
        painter: _RoomDefaultMiniCountdownPainter(
          progress: progress,
          color: widget.color,
        ),
        child: Center(
          child: Text(
            _formatTime(_interpolatedRemaining),
            style: TextStyle(
              color: widget.color,
              fontSize: 9,
              fontWeight: FontWeight.w700,
              height: 1,
            ),
          ),
        ),
      ),
    );
  }
}

class _RoomDefaultMiniCountdownPainter extends CustomPainter {
  final double progress;
  final Color color;

  _RoomDefaultMiniCountdownPainter({
    required this.progress,
    required this.color,
  });

  @override
  void paint(Canvas canvas, Size size) {
    final center = Offset(size.width / 2, size.height / 2);
    final radius = size.width / 2 - 1.5;
    const strokeWidth = 2.5;

    canvas.drawCircle(
      center,
      radius,
      Paint()
        ..color = color.withValues(alpha: 0.15)
        ..style = PaintingStyle.stroke
        ..strokeWidth = strokeWidth,
    );

    if (progress > 0) {
      canvas.drawArc(
        Rect.fromCircle(center: center, radius: radius),
        -math.pi / 2,
        2 * math.pi * progress,
        false,
        Paint()
          ..color = color.withValues(alpha: 0.7)
          ..style = PaintingStyle.stroke
          ..strokeWidth = strokeWidth
          ..strokeCap = StrokeCap.round,
      );
    }
  }

  @override
  bool shouldRepaint(covariant _RoomDefaultMiniCountdownPainter oldDelegate) =>
      progress != oldDelegate.progress || color != oldDelegate.color;
}

// ---------------------------------------------------------------------------
// Room default toggle matching the CelestialToggle from room_card.dart
// ---------------------------------------------------------------------------

class _DefaultStateToggle extends StatelessWidget {
  final _RoomDefaultMode mode;
  final ValueChanged<_RoomDefaultMode> onModeChanged;

  const _DefaultStateToggle({
    required this.mode,
    required this.onModeChanged,
  });

  void _onTap() {
    onModeChanged(_nextRoomDefaultMode(mode));
  }

  void _onLongPress() {
    if (mode != _RoomDefaultMode.off) {
      onModeChanged(_RoomDefaultMode.off);
    }
  }

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: _onTap,
      onLongPress: _onLongPress,
      behavior: HitTestBehavior.opaque,
      child: SizedBox(
        width: 64,
        height: 28,
        child: AnimatedSwitcher(
          duration: const Duration(milliseconds: 220),
          switchInCurve: Curves.easeOut,
          switchOutCurve: Curves.easeIn,
          transitionBuilder: (child, animation) => FadeTransition(
            opacity: animation,
            child: ScaleTransition(
              scale: Tween<double>(begin: 0.92, end: 1).animate(animation),
              child: child,
            ),
          ),
          child: mode == _RoomDefaultMode.none
              ? _buildNoOverrideChip()
              : _buildToggle(),
        ),
      ),
    );
  }

  Widget _buildNoOverrideChip() {
    return Container(
      key: const ValueKey('none'),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(14),
        border: Border.all(
          color: _Palette.textSecondary.withValues(alpha: 0.22),
          width: 1,
        ),
      ),
      alignment: Alignment.center,
      child: Text(
        'Auto',
        style: TextStyle(
          color: _Palette.textSecondary.withValues(alpha: 0.7),
          fontSize: 10,
          fontWeight: FontWeight.w600,
          letterSpacing: 0.4,
        ),
      ),
    );
  }

  Widget _buildToggle() {
    final alignment = switch (mode) {
      _RoomDefaultMode.off => Alignment.centerLeft,
      _RoomDefaultMode.standby => Alignment.center,
      _RoomDefaultMode.active => Alignment.centerRight,
      _RoomDefaultMode.none => Alignment.centerRight, // unreachable
    };

    final trackGradient = switch (mode) {
      _RoomDefaultMode.off || _RoomDefaultMode.none => const LinearGradient(
          colors: [Color(0xFF2A2F38), Color(0xFF30363D)],
        ),
      _RoomDefaultMode.standby => const LinearGradient(
          colors: [Color(0xFF2D2A20), Color(0xFF50472D)],
        ),
      _RoomDefaultMode.active => const LinearGradient(
          colors: [Color(0xFF8B6B20), Color(0xFFD4A020)],
        ),
    };

    final thumbColor = switch (mode) {
      _RoomDefaultMode.off || _RoomDefaultMode.none => _Palette.textSecondary,
      _RoomDefaultMode.standby => _Palette.idle,
      _RoomDefaultMode.active => Colors.white,
    };

    final thumbShadow = switch (mode) {
      _RoomDefaultMode.active => [
          BoxShadow(
            color: _Palette.amber.withValues(alpha: 0.4),
            blurRadius: 8,
            spreadRadius: 1,
          ),
        ],
      _RoomDefaultMode.standby => [
          BoxShadow(
            color: _Palette.idle.withValues(alpha: 0.28),
            blurRadius: 8,
            spreadRadius: 1,
          ),
        ],
      _RoomDefaultMode.off || _RoomDefaultMode.none => <BoxShadow>[],
    };

    return AnimatedContainer(
      key: const ValueKey('toggle'),
      duration: const Duration(milliseconds: 300),
      curve: Curves.easeInOut,
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(14),
        gradient: trackGradient,
      ),
      padding: const EdgeInsets.all(3),
      child: AnimatedAlign(
        duration: const Duration(milliseconds: 300),
        curve: Curves.easeInOut,
        alignment: alignment,
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 300),
          width: 22,
          height: 22,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: thumbColor,
            boxShadow: thumbShadow,
          ),
        ),
      ),
    );
  }
}

/// Local color constants for the room-behavior cards. Mirrors the subset of the
/// light-profile screen palette these controls use; kept private here so the
/// widget is self-contained (the values are static brand colors that don't
/// drift).
class _Palette {
  static const card = Color(0xFF13171E);
  static const border = Color(0xFF232A35);
  static const textPrimary = Color(0xFFE8EDF4);
  static const textSecondary = Color(0xFF8A919C);
  static const amber = Color(0xFFF9A825);
  static const idle = Color(0xFFB8A890);
}
