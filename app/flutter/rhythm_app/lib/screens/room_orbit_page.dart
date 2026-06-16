/// Room-specific Solar Orbit page.
///
/// A full-screen view for a single room showing:
/// - Room name header
/// - Solar orbit with power toggle
/// - Light output display
/// - Time scrubber
library;

import 'dart:async';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' show RoomModeState;
import '../api/hybrid_client.dart';
import '../providers/room_provider.dart';
import '../services/hue/hue_service_locator.dart';
import '../services/analytics_service.dart';
import '../widgets/solar_orbit.dart';
import '../widgets/time_scrubber.dart';
import '../widgets/light_output_display.dart';
import '../providers/home_provider.dart';

/// A page showing the solar orbit for a single room.
///
/// This is designed to be used inside a PageView for swipeable rooms.
class RoomOrbitPage extends StatefulWidget {
  final RoomDto room;
  final CurveConfigDto globalConfig;
  final CurveData? curveData;

  const RoomOrbitPage({
    super.key,
    required this.room,
    required this.globalConfig,
    this.curveData,
  });

  @override
  State<RoomOrbitPage> createState() => _RoomOrbitPageState();
}

class _RoomOrbitPageState extends State<RoomOrbitPage> {
  CurveData? _curveData;
  bool _isToggling = false;
  int? _manualBrightness;
  double? _dragHour; // non-null only during active drag
  Timer? _nowTimer;

  CurveConfigDto get _effectiveConfig =>
      widget.room.curveConfig ?? widget.globalConfig;

  @override
  void initState() {
    super.initState();
    _curveData = widget.curveData;
    _loadInitialState();

    // Update display periodically
    _nowTimer = Timer.periodic(const Duration(seconds: 30), (_) {
      if (mounted) setState(() {});
    });
  }

  @override
  void dispose() {
    _nowTimer?.cancel();
    super.dispose();
  }

  @override
  void didUpdateWidget(RoomOrbitPage oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.curveData != null && widget.curveData != oldWidget.curveData) {
      setState(() {
        _curveData = widget.curveData;
      });
    }
    if (widget.room.id != oldWidget.room.id) {
      _loadInitialState();
    }
  }

  void _configureHueService() {
    if (!HueServiceLocator.isDemoMode) {
      final homeProvider = context.read<HomeProvider>();
      final hueHub = homeProvider.getFirstHubOfType(HubType.hue);
      if (hueHub != null && hueHub.hasCredentials) {
        HueServiceLocator.realInstance.configure(HueConfig(
          bridgeIp: hueHub.endpoint.host,
          username: hueHub.token!,
        ));
      }
    }
  }

  Future<void> _loadInitialState() async {
    // Sync initial light state for Hue rooms
    if (widget.room.source == RoomSourceDto.hue) {
      try {
        _configureHueService();
        final isOn = await HueServiceLocator.instance.isRoomOn(widget.room.id);
        if (mounted) {
          final roomProvider = context.read<RoomProvider>();
          final room = roomProvider.getRoom(widget.room.id);
          if (room != null && room.lightsOn != isOn) {
            roomProvider.setRoomLightsOn(widget.room.id, isOn);
          }
        }
      } catch (e) {
        debugPrint('Failed to check room state: $e');
      }
    }

    // Load curve data if not provided
    if (_curveData == null) {
      _regenerateCurveData();
    }
  }

  void _regenerateCurveData() {
    final api = context.read<RhythmApi>();
    if (api is HybridApiClient) {
      final now = DateTime.now();
      final data = api.getCurveDataHighRes(
        config: _effectiveConfig,
        samplesPerHour: 4,
        date: now,
      );
      if (data != null && mounted) {
        setState(() {
          _curveData = data;
        });
      }
    }
  }

  /// Derive the displayed hour from Rust state (or drag override).
  double _displayedHour(RoomDto room) {
    if (_dragHour != null) return _dragHour!;

    final now = DateTime.now();
    final nowHour = now.hour + now.minute / 60.0;
    return ((nowHour + room.timeOffsetMinutes / 60.0) % 24.0 + 24.0) % 24.0;
  }

  void _onHourChanged(double hour) {
    setState(() {
      _dragHour = hour;
      _manualBrightness = null;
    });
  }

  /// Called when the user finishes dragging the orbital dot.
  void _onHourChangeEnd() {
    if (_dragHour == null) return;
    final draggedHour = _dragHour!;

    final roomProvider = context.read<RoomProvider>();
    final room = roomProvider.getRoom(widget.room.id);
    if (room == null) return;

    // Compute offset from real time
    final now = DateTime.now();
    final nowHour = now.hour + now.minute / 60.0;
    var offset = (draggedHour - nowHour) * 60.0;
    if (offset > 720) offset -= 1440;
    if (offset < -720) offset += 1440;

    // Pin to dragged position
    roomProvider.setRoomTimeOffset(widget.room.id, offset);
    roomProvider.setRoomRhythmEnabled(widget.room.id, false);

    setState(() {
      _dragHour = null;
    });

    // Apply lighting if on
    if (room.lightsOn) {
      _applyValuesAtHour(draggedHour);
    }
  }

  Future<void> _resetToNow() async {
    final roomProvider = context.read<RoomProvider>();
    final api = context.read<RhythmApi>();
    if (api is! HybridApiClient) return;

    final now = DateTime.now();
    final currentHour = now.hour + now.minute / 60.0;

    // Route through Rust brain
    final result = roomProvider.handleRoomAction(
      roomId: widget.room.id,
      action: RhythmActionDto.reset,
      config: _effectiveConfig,
      latitude: api.latitude,
      longitude: api.longitude,
      year: now.year,
      month: now.month,
      day: now.day,
      timezone: api.timezone,
      currentHour: currentHour,
    );

    setState(() {
      _manualBrightness = null;
      _dragHour = null;
    });

    if (result == null) return;

    // Execute returned commands (same pattern as _toggleLight)
    if (widget.room.source == RoomSourceDto.hue) {
      _configureHueService();
      for (final cmd in result.commands) {
        if (cmd.commandType == LightCommandType.turnOn) {
          final mireds = cmd.kelvin != null ? (1000000 ~/ cmd.kelvin!) : null;
          await HueServiceLocator.instance.setRoomState(
            cmd.roomId,
            on: true,
            brightness: cmd.brightness,
            mireds: mireds,
          );
        }
      }
      HapticFeedback.lightImpact();
    }
  }

  Future<void> _toggleLight() async {
    if (_isToggling) return;

    setState(() {
      _isToggling = true;
    });

    try {
      final roomProvider = context.read<RoomProvider>();
      final api = context.read<RhythmApi>();
      if (api is! HybridApiClient) return;

      final now = DateTime.now();
      final currentHour = now.hour + now.minute / 60.0;

      // Process through Rust brain
      final result = roomProvider.handleRoomAction(
        roomId: widget.room.id,
        action: RhythmActionDto.onPress,
        config: _effectiveConfig,
        latitude: api.latitude,
        longitude: api.longitude,
        year: now.year,
        month: now.month,
        day: now.day,
        timezone: api.timezone,
        currentHour: currentHour,
      );

      if (result == null) return;

      // Execute Hue commands
      switch (widget.room.source) {
        case RoomSourceDto.hue:
          _configureHueService();
          for (final cmd in result.commands) {
            if (cmd.commandType == LightCommandType.turnOn) {
              final mireds =
                  cmd.kelvin != null ? (1000000 ~/ cmd.kelvin!) : null;
              await HueServiceLocator.instance.setRoomState(
                cmd.roomId,
                on: true,
                brightness: cmd.brightness,
                mireds: mireds,
              );
            } else {
              await HueServiceLocator.instance
                  .setRoomState(cmd.roomId, on: false);
            }
          }
          HapticFeedback.mediumImpact();

          final updatedRoom = roomProvider.getRoom(widget.room.id);
          if (updatedRoom?.lightsOn ?? false) {
            roomProvider.setRoomStateLocal(
                widget.room.id, RoomModeState.active);
          }
          AnalyticsService().logLightToggle(
            turnedOn: updatedRoom?.lightsOn ?? false,
            roomId: widget.room.id,
          );
          break;
        case RoomSourceDto.matter:
        case RoomSourceDto.homeAssistant:
        case RoomSourceDto.bridge:
        case RoomSourceDto.unknown:
          break;
      }
    } catch (e) {
      debugPrint('Failed to toggle light: $e');
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: Text('Failed to toggle light: $e'),
            backgroundColor: Colors.red.shade700,
          ),
        );
      }
    } finally {
      if (mounted) {
        setState(() {
          _isToggling = false;
        });
      }
    }
  }

  void _onBrightnessChanged(int brightness) {
    setState(() {
      _manualBrightness = brightness;
    });
  }

  void _onBrightnessChangeEnd() {
    final roomProvider = context.read<RoomProvider>();
    final room = roomProvider.getRoom(widget.room.id);
    if (room == null) return;
    roomProvider.setRoomRhythmEnabled(widget.room.id, false);
    _applyValuesAtHour(_displayedHour(room));
  }

  Future<void> _applyValuesAtHour(double hour) async {
    if (widget.room.source != RoomSourceDto.hue) return;

    try {
      final api = context.read<RhythmApi>();
      if (api is! HybridApiClient) return;

      final values = api.calculateLighting(
        config: _effectiveConfig,
        currentHour: hour,
        date: DateTime.now(),
      );
      if (values == null) return;

      final brightness = _manualBrightness ?? values.brightness;

      debugPrint(
          '[Orbit] Setting room ${widget.room.name} to $brightness%, ${(1000000 / values.mireds).round()}K');
      _configureHueService();
      await HueServiceLocator.instance.setRoomState(
        widget.room.id,
        on: true,
        brightness: brightness,
        mireds: values.mireds,
      );
      if (mounted) {
        final roomProvider = context.read<RoomProvider>();
        final room = roomProvider.getRoom(widget.room.id);
        if (room != null && !room.lightsOn) {
          roomProvider.setRoomLightsOn(widget.room.id, true);
          roomProvider.setRoomStateLocal(widget.room.id, RoomModeState.active);
        }
        HapticFeedback.lightImpact();
      }
    } catch (e) {
      debugPrint('Failed to apply values: $e');
    }
  }

  int _getBrightnessAtHour(double hour) {
    if (_curveData == null) return 50;
    final data = _curveData!;
    if (data.hours.isEmpty) return 50;

    int idx = 0;
    double minDiff = double.infinity;
    for (int i = 0; i < data.hours.length; i++) {
      final diff = (data.hours[i] - hour).abs();
      if (diff < minDiff) {
        minDiff = diff;
        idx = i;
      }
    }
    return data.brightness[idx];
  }

  int _getKelvinAtHour(double hour) {
    if (_curveData == null) return 4000;
    final data = _curveData!;
    if (data.hours.isEmpty) return 4000;

    int idx = 0;
    double minDiff = double.infinity;
    for (int i = 0; i < data.hours.length; i++) {
      final diff = (data.hours[i] - hour).abs();
      if (diff < minDiff) {
        minDiff = diff;
        idx = i;
      }
    }
    return data.kelvin[idx];
  }

  @override
  Widget build(BuildContext context) {
    return Selector<RoomProvider, RoomDto?>(
      selector: (_, provider) => provider.getRoom(widget.room.id),
      builder: (context, room, child) {
        if (room == null) return const SizedBox.shrink();

        final api = context.read<RhythmApi>();
        final selectedHour = _displayedHour(room);
        final isLightOn = room.lightsOn;
        final isFollowingNow =
            room.rhythmEnabled && room.timeOffsetMinutes == 0.0;

        int curveBrightness;
        int kelvin;

        if (api is HybridApiClient) {
          final values = api.calculateLighting(
            config: _effectiveConfig,
            currentHour: selectedHour,
            date: DateTime.now(),
          );
          if (values != null) {
            curveBrightness = values.brightness;
            kelvin = values.kelvin;
          } else {
            curveBrightness = _getBrightnessAtHour(selectedHour);
            kelvin = _getKelvinAtHour(selectedHour);
          }
        } else {
          curveBrightness = _getBrightnessAtHour(selectedHour);
          kelvin = _getKelvinAtHour(selectedHour);
        }

        final brightness = _manualBrightness ?? curveBrightness;

        return SafeArea(
          bottom: false,
          child: Column(
            children: [
              // Room name header
              _buildHeader(room, isLightOn, isFollowingNow),
              // Solar orbit - main interaction area
              Expanded(
                flex: 3,
                child: Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 20),
                  child: Center(
                    child: SolarOrbit(
                      curveData: _curveData,
                      selectedHour: selectedHour,
                      isTuneMode: false,
                      isLightOn: isLightOn,
                      brightness: brightness,
                      onHourChanged: _onHourChanged,
                      onHourChangeEnd: _onHourChangeEnd,
                      onSunTap: _toggleLight,
                      onBrightnessChanged: _onBrightnessChanged,
                      onBrightnessChangeEnd: _onBrightnessChangeEnd,
                    ),
                  ),
                ),
              ),
              // Light output display
              Padding(
                padding: const EdgeInsets.fromLTRB(24, 8, 24, 0),
                child: LightOutputDisplay(
                  brightness: brightness,
                  kelvin: kelvin,
                  selectedHour: selectedHour,
                ),
              ),
              const SizedBox(height: 20),
              // Time scrubber
              Padding(
                padding: const EdgeInsets.symmetric(horizontal: 24),
                child: TimeScrubber(
                  curveData: _curveData,
                  selectedHour: selectedHour,
                  onHourChanged: _onHourChanged,
                ),
              ),
              // Bottom padding for nav overlay
              const SizedBox(height: 110),
            ],
          ),
        );
      },
    );
  }

  Widget _buildHeader(RoomDto room, bool isLightOn, bool isFollowingNow) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 12, 20, 0),
      child: Row(
        children: [
          // Room name
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  room.name,
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 20,
                    fontWeight: FontWeight.w600,
                    letterSpacing: 0.3,
                  ),
                ),
                const SizedBox(height: 2),
                Text(
                  _getSourceLabel(room.source),
                  style: TextStyle(
                    color: CelestialColors.textSecondary.withValues(alpha: 0.7),
                    fontSize: 12,
                    letterSpacing: 0.2,
                  ),
                ),
              ],
            ),
          ),
          // Now button (visible when not following real time)
          if (!isFollowingNow)
            Padding(
              padding: const EdgeInsets.only(right: 12),
              child: _buildActionButton(
                icon: Icons.my_location_rounded,
                onTap: _resetToNow,
                tooltip: 'Reset to now',
              ),
            ),
          // Power indicator
          Container(
            width: 44,
            height: 44,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: isLightOn
                  ? CelestialColors.sunWarm.withValues(alpha: 0.15)
                  : CelestialColors.backgroundCard.withValues(alpha: 0.6),
              border: Border.all(
                color: isLightOn
                    ? CelestialColors.sunWarm.withValues(alpha: 0.4)
                    : CelestialColors.orbitRing.withValues(alpha: 0.4),
                width: 1,
              ),
            ),
            child: Icon(
              isLightOn ? Icons.lightbulb : Icons.lightbulb_outline,
              color: isLightOn
                  ? CelestialColors.sunWarm
                  : CelestialColors.textSecondary.withValues(alpha: 0.8),
              size: 20,
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildActionButton({
    required IconData icon,
    required VoidCallback? onTap,
    bool isLoading = false,
    String? tooltip,
  }) {
    Widget button = GestureDetector(
      onTap: onTap,
      child: Container(
        width: 44,
        height: 44,
        decoration: BoxDecoration(
          shape: BoxShape.circle,
          color: CelestialColors.backgroundCard.withValues(alpha: 0.6),
          border: Border.all(
            color: CelestialColors.orbitRing.withValues(alpha: 0.4),
            width: 1,
          ),
        ),
        child: isLoading
            ? Padding(
                padding: const EdgeInsets.all(12),
                child: CircularProgressIndicator(
                  strokeWidth: 2,
                  color: CelestialColors.textSecondary,
                ),
              )
            : Icon(
                icon,
                color: CelestialColors.textSecondary.withValues(alpha: 0.8),
                size: 20,
              ),
      ),
    );

    if (tooltip != null) {
      button = Tooltip(message: tooltip, child: button);
    }

    return button;
  }

  String _getSourceLabel(RoomSourceDto source) {
    switch (source) {
      case RoomSourceDto.matter:
        return 'Matter';
      case RoomSourceDto.hue:
        return 'Philips Hue';
      case RoomSourceDto.homeAssistant:
        return 'Home Assistant';
      case RoomSourceDto.bridge:
        return 'Bridge';
      case RoomSourceDto.unknown:
        return 'Unknown';
    }
  }
}
