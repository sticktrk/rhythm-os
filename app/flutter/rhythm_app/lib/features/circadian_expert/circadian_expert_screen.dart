import 'dart:async';
import 'dart:math' as math;

import 'package:dio/dio.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart'
    show
        RhythmDevice,
        RhythmDeviceType,
        RhythmHello,
        RhythmRoom,
        RhythmTopologyNode,
        RoomModeState;

import '../../providers/home_provider.dart';
import '../../providers/server_sync_provider.dart';
import '../../services/server_endpoint_resolver.dart';
import '../../widgets/solar_orbit.dart' show CelestialColors;

const _expertAccent = Color(0xFFFEAC60);
const _changedAccent = Color(0xFF78AAE6);
const _dangerAccent = Color(0xFFE06C6C);
const _panelColor = Color(0xFF11151C);
const _panel2Color = Color(0xFF20242D);
const _unsetValue = Object();
const _expertProfileId = 'expert';
const _removed-projectRuntimeApi = 'api/light-runtimes/removed-circadian';
const _expertMomentExtensionKey = 'circadian_expert';
const _expertMaxReaches = 5;
const _expertKelvinStep = 250;
const _modernExpertCanManageZones = true;
const _modernExpertCanAddControlSources = true;
const _modernExpertCanEditInputBindings = true;
const _modernExpertCanEditInputBindingIdentity = true;
const _modernExpertCanEditSwitchmap = true;
const _modernExpertCanEditAreaSettings = true;
const _modernExpertCanEditAreaTuning = true;
const _modernExpertCanEditFeedbackTargets = true;
const _modernExpertCanEditLightPurposes = true;
const _modernExpertCanAssignLightSections = true;
const _modernExpertCanEditSections = true;
const _modernExpertCanEditScheduleParticipation = true;
const _modernExpertCanEditSectionTuning = true;
const _modernExpertCanSetPhaseTime = true;
const _modernExpertCanResetAreaAdjustments = true;
const _modernExpertCanSyncAreaToZone = true;
const _modernExpertCanClearSectionBrightness = true;
const _expertSettingsKeys = <String>{
  'auto_update',
  'power_recovery',
  'turn_on_transition',
  'turn_off_transition',
  'multi_click_enabled',
  'multi_click_speed',
  'environment_chain',
  'multi_area_dispatch_stagger_ms',
  'experimental_tick_mode',
  'long_press_repeat_interval',
  'motion_blink_threshold',
  'motion_warning_time',
  'off_threshold',
  'daily_sync_hour',
  'daily_sync_minute',
  'read_only_zha',
  'feedback_restrict_to_primary',
  'reach_feedback_enabled',
  'reach_daytime_threshold',
  'freeze_feedback_enabled',
  'freeze_hold_at_dim',
  'freeze_off_rise',
  'limit_bounce_enabled',
  'limit_bounce_max_percent',
  'limit_bounce_min_percent',
  'limit_warning_speed',
  'alert_bounce_speed',
  'boost_return_transition',
  'sun_saturation',
  'boost_default',
  'confirm_zone_pushes',
  'duration_picker_presets',
  'default_pause_duration_minutes',
  'default_freeze_duration_minutes',
  'default_boost_duration_minutes',
  'default_power_off_duration_minutes',
  'rhythm_cursor_step_min',
  'controls_pulse_window_hours',
  'controls_recent_window_minutes',
  'activity_log_min_entries',
  'activity_log_min_days',
  'activity_log_flush_interval_minutes',
  'tick_repeat_after_user_action',
  'tick_repeat_after_autonomous_change',
  'periodic_refresh_interval_minutes',
  'ct_comp_enabled',
  'ct_comp_begin',
  'ct_comp_end',
  'ct_comp_factor',
  'ct_compensation_enabled',
  'ct_compensation_begin_kelvin',
  'ct_compensation_end_kelvin',
  'ct_compensation_factor',
  'two_step_enabled',
  'two_step_ct_threshold',
  'two_step_bri_threshold',
  'two_step_delay',
  'two_step_kelvin_threshold',
  'two_step_brightness_threshold',
  'two_step_delay_ms',
  'warm_night_enabled',
  'warm_night_mode',
  'warm_night_target',
  'warm_night_start',
  'warm_night_end',
  'warm_night_fade',
  'daylight_enabled',
  'daylight_cct',
  'daylight_start',
  'daylight_end',
  'daylight_fade',
  'color_sensitivity',
};
const _expertProfileConfigKeys = <String>{
  'id',
  'name',
  'curve',
  'min_brightness',
  'max_brightness',
  'min_color_temp',
  'max_color_temp',
  'max_dim_steps',
  'step_fallback_minutes',
  'fade_ms',
  'motion_timeout_secs',
  'rhythm_interval_secs',
};

bool _isExpertConfigKeyWritable(String key) =>
    _expertSettingsKeys.contains(key) || _expertProfileConfigKeys.contains(key);
const _allWeekdays = [0, 1, 2, 3, 4, 5, 6];
const _momentActionOptions = [
  ('lights_on', 'On'),
  ('off', 'Off'),
  ('nitelite', 'NiteLite'),
  ('britelite', 'BriteLite'),
  ('wake_or_bed', 'Wake / Bed'),
  ('circadian_off', 'Circadian off'),
  ('reset', 'Reset'),
  ('leave_alone', 'Leave alone'),
];
const _momentCategoryOptions = [
  ('utility', 'Utility'),
  ('fun', 'Fun'),
];
const _outdoorOverrideConditions = [
  'clear',
  'partly_cloudy',
  'cloudy',
  'overcast',
  'fog',
  'rain',
  'storm',
];
const _modernWeatherGroups = [
  _ExpertWeatherGroup(key: 'clear', label: 'Clear', multiplier: 1.0),
  _ExpertWeatherGroup(
      key: 'partly_cloudy', label: 'Partly cloudy', multiplier: 0.82),
  _ExpertWeatherGroup(key: 'cloudy', label: 'Cloudy', multiplier: 0.62),
  _ExpertWeatherGroup(key: 'overcast', label: 'Overcast', multiplier: 0.45),
  _ExpertWeatherGroup(key: 'fog', label: 'Fog', multiplier: 0.38),
  _ExpertWeatherGroup(key: 'rain', label: 'Rain', multiplier: 0.35),
  _ExpertWeatherGroup(key: 'storm', label: 'Storm', multiplier: 0.22),
];
const _defaultRhythmPresetOptions = [
  'young',
  'adult',
  'nightowl',
  'duskbat',
  'shiftearly',
  'shiftlate',
];
const _momentIconOptions = [
  ('&#128716;', 'Bed'),
  ('&#127769;', 'Moon'),
  ('&#127763;', 'Moon face'),
  ('&#127774;', 'Sun'),
  ('&#127773;', 'Sun rays'),
  ('&#127749;', 'Sunset'),
  ('&#127747;', 'Sunrise'),
  ('&#127752;', 'Rainbow'),
  ('&#127881;', 'Party'),
  ('&#127878;', 'Celebrate'),
  ('&#127871;', 'Candle'),
  ('&#128161;', 'Bulb'),
  ('&#127968;', 'Home'),
  ('&#128682;', 'Door'),
  ('&#128249;', 'Movie'),
  ('&#127909;', 'Clapperboard'),
  ('&#127911;', 'Headphones'),
  ('&#127926;', 'Music'),
  ('&#127860;', 'Dinner'),
  ('&#9749;', 'Coffee'),
  ('&#128218;', 'Book'),
  ('&#128187;', 'Work'),
  ('&#128170;', 'Workout'),
  ('&#128704;', 'Yoga'),
  ('&#9889;', 'Bright'),
  ('&#128680;', 'Emergency'),
  ('&#9211;', 'Off'),
];

class CircadianExpertScreen extends StatefulWidget {
  final bool showBackButton;
  final bool showToolStrip;

  const CircadianExpertScreen({
    super.key,
    this.showBackButton = true,
    this.showToolStrip = true,
  });

  static Future<void> show(BuildContext context) {
    return Navigator.of(context).push(
      PageRouteBuilder<void>(
        transitionDuration: const Duration(milliseconds: 650),
        reverseTransitionDuration: const Duration(milliseconds: 420),
        pageBuilder: (_, __, ___) => const CircadianExpertScreen(),
        transitionsBuilder: (_, animation, secondaryAnimation, child) {
          final curved = CurvedAnimation(
            parent: animation,
            curve: Curves.easeOutBack,
            reverseCurve: Curves.easeInCubic,
          );
          return AnimatedBuilder(
            animation: curved,
            child: child,
            builder: (context, child) {
              final angle = (1 - curved.value) * math.pi * 0.5;
              return Transform(
                alignment: Alignment.center,
                transform: Matrix4.identity()
                  ..setEntry(3, 2, 0.0015)
                  ..rotateY(angle),
                child: child,
              );
            },
          );
        },
      ),
    );
  }

  @override
  State<CircadianExpertScreen> createState() => _CircadianExpertScreenState();
}

class _CircadianExpertScreenState extends State<CircadianExpertScreen> {
  late List<_ExpertZone> _zones = _demoZones();
  List<String> _serverOrder = const [];
  _CircadianExpertClient? _client;
  _ExpertServerSnapshot? _serverSnapshot;
  bool _isLoadingServer = false;
  String _serverStatus = 'Demo data';
  bool _manageMode = false;
  final Map<String, bool> _areaPowerOverrides = {};
  final Map<String, double> _areaBrightnessOverrides = {};
  final Set<String> _areaBusyIds = {};

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) unawaited(_loadServerZones());
    });
  }

  Future<void> _loadServerZones({bool silent = false}) async {
    if (!silent) {
      setState(() {
        _isLoadingServer = true;
        _serverStatus = 'Connecting to RhythmOS...';
      });
    }

    try {
      final client = await _CircadianExpertClient.resolve(context);
      if (client == null) {
        if (!mounted) return;
        setState(() {
          _client = null;
          _serverSnapshot = null;
          _serverOrder = const [];
          _areaPowerOverrides.clear();
          _areaBrightnessOverrides.clear();
          _areaBusyIds.clear();
          _serverStatus = 'Demo data - no RhythmOS server selected';
          _isLoadingServer = false;
        });
        return;
      }

      final result = await client.fetchZones();
      final snapshot = await client.fetchServerSnapshot();
      if (!mounted) return;
      setState(() {
        _client = client;
        _serverSnapshot = snapshot;
        _serverOrder = result.allOrder;
        _areaPowerOverrides.clear();
        _areaBrightnessOverrides.clear();
        if (result.visibleZones.isNotEmpty) {
          _zones = result.visibleZones;
        }
        _serverStatus = 'Connected to ${client.baseUrlLabel}';
        _isLoadingServer = false;
      });
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: server load failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() {
        _client = null;
        _serverSnapshot = null;
        _serverOrder = const [];
        _areaPowerOverrides.clear();
        _areaBrightnessOverrides.clear();
        _areaBusyIds.clear();
        _serverStatus = 'Demo data - circadian endpoints unavailable';
        _isLoadingServer = false;
      });
    }
  }

  Future<void> _runServerMutation(
    Future<void> Function(_CircadianExpertClient client) action, {
    bool reload = false,
  }) async {
    final client = _client;
    if (client == null) return;
    try {
      await action(client);
      if (reload) {
        await _loadServerZones(silent: true);
      }
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: server mutation failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('Expert server update failed: $error')),
      );
    }
  }

  void _toggleManage() {
    HapticFeedback.selectionClick();
    setState(() => _manageMode = !_manageMode);
  }

  bool _areaPowerValue(_ExpertArea area) {
    return _areaPowerOverrides[area.id] ??
        area.lightsOn ??
        (area.brightness == null || area.brightness! > 0);
  }

  double _areaBrightnessValue(_ExpertArea area, _ExpertZone zone) {
    final stateBrightness =
        zone.currentState?.actualBrightness ?? zone.currentState?.brightness;
    final value = _areaBrightnessOverrides[area.id] ??
        area.brightness?.toDouble() ??
        stateBrightness?.toDouble() ??
        50.0;
    return value.clamp(1, 100).toDouble();
  }

  void _toggleAreaPower(_ExpertArea area) {
    final next = !_areaPowerValue(area);
    unawaited(_setAreaPower(area, next));
  }

  Future<void> _setAreaPower(_ExpertArea area, bool powerOn) async {
    final client = _client;
    if (client == null) return;
    final previous = _areaPowerOverrides[area.id];

    HapticFeedback.selectionClick();
    setState(() {
      _areaPowerOverrides[area.id] = powerOn;
      _areaBusyIds.add(area.id);
    });

    try {
      await client.runAreaAction(
        area.id,
        powerOn ? 'lights_on' : 'lights_off',
      );
      await _loadServerZones(silent: true);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: area power failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() {
        if (previous == null) {
          _areaPowerOverrides.remove(area.id);
        } else {
          _areaPowerOverrides[area.id] = previous;
        }
      });
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('Room power update failed: $error')),
      );
    } finally {
      if (mounted) {
        setState(() => _areaBusyIds.remove(area.id));
      }
    }
  }

  void _stepAreaBrightness(_ExpertArea area, String direction) {
    unawaited(_runAreaBrightnessStep(area, direction));
  }

  Future<void> _runAreaBrightnessStep(
    _ExpertArea area,
    String direction,
  ) async {
    final client = _client;
    if (client == null) return;
    final action = direction == 'down' ? 'bright_down' : 'bright_up';

    HapticFeedback.selectionClick();
    setState(() => _areaBusyIds.add(area.id));

    try {
      await client.runAreaAction(area.id, action);
      await _loadServerZones(silent: true);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: area brightness step failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('Room brightness step failed: $error')),
      );
    } finally {
      if (mounted) {
        setState(() => _areaBusyIds.remove(area.id));
      }
    }
  }

  void _updateAreaBrightnessDraft(_ExpertArea area, double value) {
    setState(() {
      _areaBrightnessOverrides[area.id] = value.clamp(1, 100).toDouble();
    });
  }

  void _commitAreaBrightness(_ExpertArea area, double value) {
    unawaited(_setAreaBrightness(area, value));
  }

  Future<void> _setAreaBrightness(_ExpertArea area, double value) async {
    final client = _client;
    if (client == null) return;
    final brightness = value.round().clamp(1, 100).toInt();
    final previous = _areaBrightnessOverrides[area.id];

    setState(() {
      _areaBrightnessOverrides[area.id] = brightness.toDouble();
      _areaBusyIds.add(area.id);
    });

    try {
      await client.setAreaBrightness(area.id, brightness);
      await _loadServerZones(silent: true);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: area brightness failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() {
        if (previous == null) {
          _areaBrightnessOverrides.remove(area.id);
        } else {
          _areaBrightnessOverrides[area.id] = previous;
        }
      });
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('Room brightness update failed: $error')),
      );
    } finally {
      if (mounted) {
        setState(() => _areaBusyIds.remove(area.id));
      }
    }
  }

  void _moveZone(int index, int delta) {
    final next = index + delta;
    if (next < 0 || next >= _zones.length) return;
    HapticFeedback.selectionClick();
    setState(() {
      final zone = _zones.removeAt(index);
      _zones.insert(next, zone);
    });
    unawaited(
      _runServerMutation(
        (client) => client.reorderZones(_serverOrderWithVisibleZones()),
      ),
    );
  }

  void _moveAreaInZone(int zoneIndex, int areaIndex, int delta) {
    final zone = _zones[zoneIndex];
    final next = areaIndex + delta;
    if (next < 0 || next >= zone.areas.length) return;

    final areas = List<_ExpertArea>.from(zone.areas);
    final area = areas.removeAt(areaIndex);
    areas.insert(next, area);

    HapticFeedback.selectionClick();
    setState(() {
      _zones[zoneIndex] = zone.copyWith(areas: areas);
    });
    unawaited(
      _runServerMutation(
        (client) => client.reorderZoneAreas(
          zone.name,
          areas.map((area) => area.id),
        ),
        reload: true,
      ),
    );
  }

  void _moveAreaToZone(
    int sourceZoneIndex,
    int areaIndex,
    String targetZoneName,
  ) {
    final targetZoneIndex =
        _zones.indexWhere((zone) => zone.name == targetZoneName);
    if (targetZoneIndex < 0 || targetZoneIndex == sourceZoneIndex) return;

    final sourceZone = _zones[sourceZoneIndex];
    if (areaIndex < 0 || areaIndex >= sourceZone.areas.length) return;

    final area = sourceZone.areas[areaIndex];
    final sourceAreas = List<_ExpertArea>.from(sourceZone.areas)
      ..removeAt(areaIndex);
    final targetZone = _zones[targetZoneIndex];
    final targetAreas = [
      ...targetZone.areas.where((candidate) => candidate.id != area.id),
      area,
    ];

    HapticFeedback.selectionClick();
    setState(() {
      _zones[sourceZoneIndex] = sourceZone.copyWith(areas: sourceAreas);
      _zones[targetZoneIndex] = targetZone.copyWith(areas: targetAreas);
    });
    unawaited(
      _runServerMutation(
        (client) => client.moveAreaToZone(targetZoneName, area),
        reload: true,
      ),
    );
  }

  void _removeAreaFromZone(int zoneIndex, int areaIndex) {
    final sourceZone = _zones[zoneIndex];
    if (sourceZone.isDefault) return;
    if (areaIndex < 0 || areaIndex >= sourceZone.areas.length) return;

    final defaultZoneIndex = _zones.indexWhere((zone) => zone.isDefault);
    final area = sourceZone.areas[areaIndex];
    final sourceAreas = List<_ExpertArea>.from(sourceZone.areas)
      ..removeAt(areaIndex);

    HapticFeedback.selectionClick();
    setState(() {
      _zones[zoneIndex] = sourceZone.copyWith(areas: sourceAreas);
      if (defaultZoneIndex >= 0 && defaultZoneIndex != zoneIndex) {
        final defaultZone = _zones[defaultZoneIndex];
        _zones[defaultZoneIndex] = defaultZone.copyWith(
          areas: [
            ...defaultZone.areas.where((candidate) => candidate.id != area.id),
            area,
          ],
        );
      }
    });
    unawaited(
      _runServerMutation(
        (client) => client.removeAreaFromZone(sourceZone.name, area.id),
        reload: true,
      ),
    );
  }

  Future<void> _purgeStaleArea(int zoneIndex, int areaIndex) async {
    final zone = _zones[zoneIndex];
    if (areaIndex < 0 || areaIndex >= zone.areas.length) return;
    final area = zone.areas[areaIndex];
    if (!area.stale) return;

    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: const Text(
          'Remove stale area',
          style: TextStyle(color: CelestialColors.textPrimary),
        ),
        content: Text(
          'Remove "${area.name}" from expert zones and control scopes? This is intended for areas removed from Home Assistant.',
          style: const TextStyle(color: CelestialColors.textSecondary),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('Cancel'),
          ),
          TextButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text(
              'Remove',
              style: TextStyle(color: _dangerAccent),
            ),
          ),
        ],
      ),
    );
    if (confirmed != true) return;

    HapticFeedback.selectionClick();
    setState(() {
      _zones = [
        for (final candidate in _zones)
          candidate.copyWith(
            areas: [
              for (final candidateArea in candidate.areas)
                if (candidateArea.id != area.id) candidateArea,
            ],
          ),
      ];
    });
    unawaited(
      _runServerMutation(
        (client) => client.purgeAreaFromConfig(area.id),
        reload: true,
      ),
    );
  }

  Future<void> _renameZone(int index) async {
    final current = _zones[index].name;
    final controller = TextEditingController(text: current);
    final value = await showDialog<String>(
      context: context,
      builder: (context) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: const Text(
          'Rename zone',
          style: TextStyle(color: CelestialColors.textPrimary),
        ),
        content: TextField(
          controller: controller,
          autofocus: true,
          style: const TextStyle(color: CelestialColors.textPrimary),
          decoration: const InputDecoration(
            labelText: 'Zone name',
            labelStyle: TextStyle(color: CelestialColors.textSecondary),
            enabledBorder: UnderlineInputBorder(
              borderSide: BorderSide(color: CelestialColors.orbitRing),
            ),
            focusedBorder: UnderlineInputBorder(
              borderSide: BorderSide(color: _expertAccent),
            ),
          ),
          onSubmitted: (value) => Navigator.of(context).pop(value),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(),
            child: const Text('Cancel'),
          ),
          TextButton(
            onPressed: () => Navigator.of(context).pop(controller.text),
            child: const Text('Save'),
          ),
        ],
      ),
    );
    controller.dispose();
    final trimmed = value?.trim();
    if (trimmed == null || trimmed.isEmpty || trimmed == current) return;
    setState(() {
      _zones[index] = _zones[index].copyWith(id: trimmed, name: trimmed);
    });
    unawaited(
      _runServerMutation(
        (client) => client.renameZone(current, trimmed),
        reload: true,
      ),
    );
  }

  void _makeDefault(int index) {
    HapticFeedback.selectionClick();
    final name = _zones[index].name;
    setState(() {
      _zones = [
        for (int i = 0; i < _zones.length; i++)
          _zones[i].copyWith(isDefault: i == index),
      ];
    });
    unawaited(
      _runServerMutation(
        (client) => client.setDefaultZone(name),
        reload: true,
      ),
    );
  }

  void _deleteZone(int index) {
    if (_zones.length <= 1) return;
    HapticFeedback.selectionClick();
    final name = _zones[index].name;
    setState(() {
      final removedDefault = _zones[index].isDefault;
      _zones.removeAt(index);
      if (removedDefault && _zones.isNotEmpty) {
        _zones[0] = _zones[0].copyWith(isDefault: true);
      }
    });
    unawaited(
      _runServerMutation(
        (client) => client.deleteZone(name),
        reload: true,
      ),
    );
  }

  void _createZone() {
    HapticFeedback.selectionClick();
    final n = _zones.length + 1;
    final name = 'New rhythm zone $n';
    setState(() {
      _zones.add(
        _ExpertZone(
          name: name,
          isDefault: _zones.isEmpty,
          areas: const [],
          definition: _RhythmDefinition.defaults(),
        ),
      );
      _manageMode = true;
    });
    unawaited(
      _runServerMutation(
        (client) => client.createZone(name),
        reload: true,
      ),
    );
  }

  void _updateZone(_ExpertZone zone) {
    final index = _zones.indexWhere((candidate) => candidate.id == zone.id);
    if (index < 0) return;
    setState(() => _zones[index] = zone);
  }

  void _openLive(_ExpertZone zone) {
    Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) => _ExpertZoneLiveScreen(
          zone: zone,
          allZones: _zones,
          client: _client,
          onZoneChanged: _updateZone,
        ),
      ),
    );
  }

  void _openDefine(_ExpertZone zone) {
    Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) => _ExpertZoneDefineScreen(
          zone: zone,
          allZones: _zones,
          client: _client,
          onZoneChanged: _updateZone,
        ),
      ),
    );
  }

  void _openActivity() {
    Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) => _ExpertActivityScreen(
          client: _client,
          zones: _zones,
        ),
      ),
    );
  }

  void _openMoments() {
    Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) => _ExpertMomentsScreen(
          client: _client,
          zones: _zones,
        ),
      ),
    );
  }

  void _openSwitches() {
    Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) => _ExpertSwitchesScreen(
          client: _client,
          zones: _zones,
        ),
      ),
    );
  }

  void _openExpertSettings() {
    Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) => _ExpertSettingsScreen(client: _client),
      ),
    );
  }

  List<String> _serverOrderWithVisibleZones() {
    final visibleNames = _zones.map((zone) => zone.name).toList();
    if (_serverOrder.isEmpty) return visibleNames;

    var visibleIndex = 0;
    final order = <String>[];
    for (final name in _serverOrder) {
      if (name.startsWith('__')) {
        order.add(name);
      } else if (visibleIndex < visibleNames.length) {
        order.add(visibleNames[visibleIndex]);
        visibleIndex++;
      }
    }
    while (visibleIndex < visibleNames.length) {
      order.add(visibleNames[visibleIndex]);
      visibleIndex++;
    }
    return order;
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            _ExpertHeader(
              title: 'Rhythm zones',
              leading: widget.showBackButton
                  ? _BackButton(onPressed: () => Navigator.of(context).pop())
                  : const _ExpertModeBadge(),
              trailing: _modernExpertCanManageZones
                  ? TextButton(
                      onPressed: _toggleManage,
                      style: TextButton.styleFrom(
                        foregroundColor: _manageMode
                            ? _expertAccent
                            : CelestialColors.textPrimary,
                        backgroundColor: _manageMode
                            ? _expertAccent.withValues(alpha: 0.14)
                            : _panel2Color,
                        side: BorderSide(
                          color: _manageMode
                              ? _expertAccent
                              : CelestialColors.orbitRing,
                        ),
                        shape: RoundedRectangleBorder(
                          borderRadius: BorderRadius.circular(8),
                        ),
                      ),
                      child: Text(_manageMode ? 'Done' : 'Manage'),
                    )
                  : const SizedBox.shrink(),
            ),
            Expanded(
              child: ListView(
                padding: const EdgeInsets.fromLTRB(20, 4, 20, 32),
                children: [
                  _ExpertServerStatus(
                    status: _serverStatus,
                    connected: _client != null,
                    loading: _isLoadingServer,
                    snapshot: _serverSnapshot,
                    onRefresh: _loadServerZones,
                  ),
                  const SizedBox(height: 10),
                  if (widget.showToolStrip) ...[
                    _ExpertToolStrip(
                      onActivity: _openActivity,
                      onMoments: _openMoments,
                      onSwitches: _openSwitches,
                      onSettings: _openExpertSettings,
                    ),
                    const SizedBox(height: 16),
                  ] else
                    const SizedBox(height: 10),
                  for (int i = 0; i < _zones.length; i++)
                    Padding(
                      padding: const EdgeInsets.only(bottom: 10),
                      child: _manageMode
                          ? Column(
                              children: [
                                _ExpertZoneManageRow(
                                  zone: _zones[i],
                                  canMoveUp: i > 0,
                                  canMoveDown: i < _zones.length - 1,
                                  canDelete: _zones.length > 1,
                                  onMoveUp: () => _moveZone(i, -1),
                                  onMoveDown: () => _moveZone(i, 1),
                                  onRename: () => _renameZone(i),
                                  onMakeDefault: () => _makeDefault(i),
                                  onDelete: () => _deleteZone(i),
                                ),
                                _ExpertZoneAreaManageList(
                                  zone: _zones[i],
                                  zoneNames: [
                                    for (final zone in _zones) zone.name,
                                  ],
                                  onMoveUp: (areaIndex) =>
                                      _moveAreaInZone(i, areaIndex, -1),
                                  onMoveDown: (areaIndex) =>
                                      _moveAreaInZone(i, areaIndex, 1),
                                  onMoveToZone: (areaIndex, targetZoneName) =>
                                      _moveAreaToZone(
                                    i,
                                    areaIndex,
                                    targetZoneName,
                                  ),
                                  onRemoveFromZone: (areaIndex) =>
                                      _removeAreaFromZone(i, areaIndex),
                                  onPurgeStaleArea: (areaIndex) =>
                                      unawaited(_purgeStaleArea(i, areaIndex)),
                                ),
                              ],
                            )
                          : _ExpertZoneNavRow(
                              zone: _zones[i],
                              connected: _client != null,
                              areaBusyIds: _areaBusyIds,
                              areaPowerValue: _areaPowerValue,
                              areaBrightnessValue: (area) =>
                                  _areaBrightnessValue(area, _zones[i]),
                              onAreaPowerToggle: _toggleAreaPower,
                              onAreaBrightnessChanged:
                                  _updateAreaBrightnessDraft,
                              onAreaBrightnessChangeEnd: _commitAreaBrightness,
                              onAreaBrightnessStep: _stepAreaBrightness,
                              onLive: () => _openLive(_zones[i]),
                              onDefine: () => _openDefine(_zones[i]),
                            ),
                    ),
                  if (_manageMode)
                    OutlinedButton.icon(
                      onPressed: _createZone,
                      icon: const Icon(Icons.add_rounded),
                      label: const Text('New zone'),
                      style: OutlinedButton.styleFrom(
                        foregroundColor: _expertAccent,
                        side: BorderSide(
                          color:
                              CelestialColors.orbitRing.withValues(alpha: 0.9),
                          style: BorderStyle.solid,
                        ),
                        shape: RoundedRectangleBorder(
                          borderRadius: BorderRadius.circular(10),
                        ),
                        padding: const EdgeInsets.symmetric(vertical: 14),
                      ),
                    ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class CircadianExpertRhythmScreen extends StatelessWidget {
  const CircadianExpertRhythmScreen({super.key});

  @override
  Widget build(BuildContext context) {
    return _ExpertServerScope(
      builder: (context, client, zones, loading, refresh) {
        if (zones.isEmpty) {
          return _ExpertEmptyTabScaffold(
            title: 'Rhythm',
            loading: loading,
            message: 'No rhythm zones found.',
            onRefresh: refresh,
          );
        }
        final defaultIndex = zones.indexWhere((zone) => zone.isDefault);
        final zone = zones[defaultIndex >= 0 ? defaultIndex : 0];
        return _ExpertZoneDefineScreen(
          zone: zone,
          allZones: zones,
          client: client,
          showBackButton: false,
          onZoneChanged: (_) => refresh(),
        );
      },
    );
  }
}

class CircadianExpertMomentsTabScreen extends StatelessWidget {
  const CircadianExpertMomentsTabScreen({super.key});

  @override
  Widget build(BuildContext context) {
    return _ExpertServerScope(
      builder: (context, client, zones, loading, refresh) {
        if (loading) {
          return _ExpertEmptyTabScaffold(
            title: 'Moments',
            loading: true,
            message: 'Loading expert moments...',
            onRefresh: refresh,
          );
        }
        return _ExpertMomentsScreen(
          client: client,
          zones: zones,
          showBackButton: false,
        );
      },
    );
  }
}

class CircadianExpertControlsTabScreen extends StatelessWidget {
  const CircadianExpertControlsTabScreen({super.key});

  @override
  Widget build(BuildContext context) {
    return _ExpertServerScope(
      builder: (context, client, zones, loading, refresh) {
        if (loading) {
          return _ExpertEmptyTabScaffold(
            title: 'Controls',
            loading: true,
            message: 'Loading expert controls...',
            onRefresh: refresh,
          );
        }
        return _ExpertSwitchesScreen(
          client: client,
          zones: zones,
          showBackButton: false,
        );
      },
    );
  }
}

typedef _ExpertServerScopeBuilder = Widget Function(
  BuildContext context,
  _CircadianExpertClient? client,
  List<_ExpertZone> zones,
  bool loading,
  VoidCallback refresh,
);

class _ExpertServerScope extends StatefulWidget {
  final _ExpertServerScopeBuilder builder;

  const _ExpertServerScope({required this.builder});

  @override
  State<_ExpertServerScope> createState() => _ExpertServerScopeState();
}

class _ExpertServerScopeState extends State<_ExpertServerScope> {
  _CircadianExpertClient? _client;
  List<_ExpertZone> _zones = _demoZones();
  bool _loading = true;

  @override
  void initState() {
    super.initState();
    unawaited(_load());
  }

  Future<void> _load() async {
    setState(() => _loading = true);
    try {
      final client = await _CircadianExpertClient.resolve(context);
      if (client == null) {
        if (!mounted) return;
        setState(() {
          _client = null;
          _zones = _demoZones();
          _loading = false;
        });
        return;
      }
      final result = await client.fetchZones();
      if (!mounted) return;
      setState(() {
        _client = client;
        _zones = result.visibleZones;
        _loading = false;
      });
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: expert tab zone load failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() {
        _client = null;
        _zones = _demoZones();
        _loading = false;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    return widget.builder(
      context,
      _client,
      _zones,
      _loading,
      () => unawaited(_load()),
    );
  }
}

class _ExpertEmptyTabScaffold extends StatelessWidget {
  final String title;
  final bool loading;
  final String message;
  final VoidCallback onRefresh;

  const _ExpertEmptyTabScaffold({
    required this.title,
    required this.loading,
    required this.message,
    required this.onRefresh,
  });

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            _ExpertHeader(
              title: title,
              leading: const _ExpertModeBadge(),
              trailing: IconButton(
                onPressed: loading ? null : onRefresh,
                tooltip: 'Refresh $title',
                icon: const Icon(Icons.refresh_rounded),
                color: CelestialColors.textSecondary,
              ),
            ),
            Expanded(
              child: Center(
                child: Padding(
                  padding: const EdgeInsets.all(24),
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      if (loading)
                        const CircularProgressIndicator(strokeWidth: 2)
                      else
                        const Icon(
                          Icons.tune_rounded,
                          color: _expertAccent,
                          size: 34,
                        ),
                      const SizedBox(height: 14),
                      Text(
                        message,
                        textAlign: TextAlign.center,
                        style: const TextStyle(
                          color: CelestialColors.textSecondary,
                          fontSize: 14,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _ExpertActivityScreen extends StatefulWidget {
  final _CircadianExpertClient? client;
  final List<_ExpertZone> zones;

  const _ExpertActivityScreen({
    required this.client,
    required this.zones,
  });

  @override
  State<_ExpertActivityScreen> createState() => _ExpertActivityScreenState();
}

class _ExpertActivityScreenState extends State<_ExpertActivityScreen> {
  _ExpertActivityFeed _feed = const _ExpertActivityFeed.empty();
  bool _loading = true;
  String? _error;
  String _areaFilter = '';
  String _sourceFilter = '';
  String _actionFilter = '';
  String _sortMode = 'recent';

  @override
  void initState() {
    super.initState();
    unawaited(_loadActivity());
  }

  Map<String, String> get _areaNames {
    final names = <String, String>{'__home__': 'Home'};
    for (final zone in widget.zones) {
      for (final area in zone.areas) {
        names[area.id] = area.name;
      }
    }
    return names;
  }

  Future<void> _loadActivity() async {
    final client = widget.client;
    if (client == null) {
      setState(() {
        _loading = false;
        _error = 'Connect to RhythmOS to view expert activity.';
      });
      return;
    }

    setState(() {
      _loading = true;
      _error = null;
    });

    try {
      final feed = await client.fetchActivity(
        area: _areaFilter.isEmpty ? null : _areaFilter,
        source: _sourceFilter.isEmpty ? null : _sourceFilter,
        action: _actionFilter.isEmpty ? null : _actionFilter,
      );
      if (!mounted) return;
      setState(() {
        _feed = feed;
        _loading = false;
      });
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: activity load failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() {
        _loading = false;
        _error = 'Activity load failed: $error';
      });
    }
  }

  void _setAreaFilter(String value) {
    setState(() => _areaFilter = value);
    unawaited(_loadActivity());
  }

  void _setSourceFilter(String value) {
    setState(() => _sourceFilter = value);
    unawaited(_loadActivity());
  }

  void _setActionFilter(String value) {
    setState(() => _actionFilter = value);
    unawaited(_loadActivity());
  }

  List<_ExpertActivityGroup> _groups() {
    final entries = _sortedEntries();
    if (_sortMode == 'recent') {
      final buckets = <String, List<_ExpertActivityEntry>>{};
      for (final entry in entries) {
        (buckets[_activityDateBucket(entry.ts)] ??= []).add(entry);
      }
      return [
        for (final key in ['Today', 'Yesterday', 'This week', 'Older'])
          if ((buckets[key] ?? const []).isNotEmpty)
            _ExpertActivityGroup(label: key, entries: buckets[key]!),
      ];
    }

    final grouped = <String, List<_ExpertActivityEntry>>{};
    for (final entry in entries) {
      final key = switch (_sortMode) {
        'area' => _areaNames[entry.areaId] ?? entry.areaId ?? 'Unassigned',
        'source' => _sourceKindLabel(entry.sourceKind),
        'action' => _activityActionLabel(entry.action),
        _ => 'Activity',
      };
      (grouped[key] ??= []).add(entry);
    }
    final labels = grouped.keys.toList()..sort();
    return [
      for (final label in labels)
        _ExpertActivityGroup(label: label, entries: grouped[label]!),
    ];
  }

  List<_ExpertActivityEntry> _sortedEntries() {
    final entries = List<_ExpertActivityEntry>.from(_feed.entries);
    if (_sortMode == 'recent') return entries;
    entries.sort((a, b) {
      final primaryA = switch (_sortMode) {
        'area' => _areaNames[a.areaId] ?? a.areaId ?? '',
        'source' => _sourceKindLabel(a.sourceKind),
        'action' => _activityActionLabel(a.action),
        _ => '',
      };
      final primaryB = switch (_sortMode) {
        'area' => _areaNames[b.areaId] ?? b.areaId ?? '',
        'source' => _sourceKindLabel(b.sourceKind),
        'action' => _activityActionLabel(b.action),
        _ => '',
      };
      final compare = primaryA.compareTo(primaryB);
      return compare != 0 ? compare : b.ts.compareTo(a.ts);
    });
    return entries;
  }

  @override
  Widget build(BuildContext context) {
    final areaItems = [
      '',
      ...(_areaNames.entries.toList()
            ..sort((a, b) => a.value.compareTo(b.value)))
          .map((entry) => entry.key),
    ];
    final sourceItems = ['', ..._feed.sources];
    final actionItems = ['', ..._feed.actions];
    final groups = _groups();

    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            _ExpertHeader(
              title: 'Activity',
              leading:
                  _BackButton(onPressed: () => Navigator.of(context).pop()),
              trailing: IconButton(
                onPressed: _loading ? null : _loadActivity,
                tooltip: 'Refresh activity',
                icon: const Icon(Icons.refresh_rounded),
                color: CelestialColors.textSecondary,
                style: IconButton.styleFrom(
                  backgroundColor: _panel2Color,
                  fixedSize: const Size(40, 40),
                  minimumSize: const Size(40, 40),
                  shape: RoundedRectangleBorder(
                    borderRadius: BorderRadius.circular(8),
                  ),
                  side: const BorderSide(color: CelestialColors.orbitRing),
                ),
              ),
            ),
            Expanded(
              child: ListView(
                padding: const EdgeInsets.fromLTRB(16, 0, 16, 28),
                children: [
                  _ExpertPanel(
                    padding: const EdgeInsets.all(12),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        Row(
                          children: [
                            Expanded(
                              child: Wrap(
                                spacing: 8,
                                runSpacing: 8,
                                children: [
                                  _CompactDropdown<String>(
                                    value: _areaFilter,
                                    items: areaItems,
                                    itemLabel: (id) => id.isEmpty
                                        ? 'Areas'
                                        : _areaNames[id] ?? id,
                                    onChanged: _setAreaFilter,
                                  ),
                                  _CompactDropdown<String>(
                                    value: _sourceFilter,
                                    items: sourceItems,
                                    itemLabel: (source) => source.isEmpty
                                        ? 'Sources'
                                        : _sourceKindLabel(source),
                                    onChanged: _setSourceFilter,
                                  ),
                                  _CompactDropdown<String>(
                                    value: _actionFilter,
                                    items: actionItems,
                                    itemLabel: (action) => action.isEmpty
                                        ? 'Actions'
                                        : _activityActionLabel(action),
                                    onChanged: _setActionFilter,
                                  ),
                                ],
                              ),
                            ),
                            if (_loading) ...[
                              const SizedBox(width: 10),
                              const SizedBox(
                                width: 18,
                                height: 18,
                                child:
                                    CircularProgressIndicator(strokeWidth: 2),
                              ),
                            ],
                          ],
                        ),
                        const SizedBox(height: 12),
                        _ActivitySortChips(
                          value: _sortMode,
                          onChanged: (value) =>
                              setState(() => _sortMode = value),
                        ),
                        if (_error != null) ...[
                          const SizedBox(height: 10),
                          Text(
                            _error!,
                            style: const TextStyle(
                              color: _dangerAccent,
                              fontSize: 12,
                              fontWeight: FontWeight.w600,
                            ),
                          ),
                        ],
                      ],
                    ),
                  ),
                  const SizedBox(height: 12),
                  if (!_loading && _feed.entries.isEmpty)
                    _ExpertPanel(
                      child: Text(
                        _hasActivityFilter
                            ? 'No activity matches the current filters.'
                            : 'No activity yet.',
                        style: const TextStyle(
                          color: CelestialColors.textSecondary,
                          fontSize: 13,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                    )
                  else
                    for (final group in groups) ...[
                      _ActivityGroupHeader(
                        label: group.label,
                        count: group.entries.length,
                      ),
                      for (final entry in group.entries)
                        _ActivityEntryRow(
                          entry: entry,
                          areaName: _areaNames[entry.areaId] ?? entry.areaId,
                        ),
                    ],
                  if (_feed.capped && _feed.entries.isNotEmpty)
                    Padding(
                      padding: const EdgeInsets.only(top: 12),
                      child: Text(
                        'Showing latest ${_feed.entries.length}; narrow filters to see older activity.',
                        style: const TextStyle(
                          color: CelestialColors.textSecondary,
                          fontSize: 11,
                        ),
                      ),
                    ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }

  bool get _hasActivityFilter =>
      _areaFilter.isNotEmpty ||
      _sourceFilter.isNotEmpty ||
      _actionFilter.isNotEmpty;
}

class _ExpertSettingsScreen extends StatefulWidget {
  final _CircadianExpertClient? client;

  const _ExpertSettingsScreen({required this.client});

  @override
  State<_ExpertSettingsScreen> createState() => _ExpertSettingsScreenState();
}

class _ExpertSettingsScreenState extends State<_ExpertSettingsScreen> {
  _ExpertConfig _config = const _ExpertConfig.empty();
  _ExpertOutdoorStatus? _outdoorStatus;
  bool _loading = true;
  bool _outdoorLoading = false;
  bool _saving = false;
  String? _savingKey;
  String? _outdoorSavingKey;
  String? _error;

  @override
  void initState() {
    super.initState();
    unawaited(_loadConfig());
  }

  Future<void> _loadConfig() async {
    final client = widget.client;
    if (client == null) {
      setState(() {
        _loading = false;
        _error = 'Connect to RhythmOS to edit expert settings.';
      });
      return;
    }

    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final config = await client.fetchExpertConfig();
      _ExpertOutdoorStatus? outdoorStatus;
      try {
        outdoorStatus = await client.fetchOutdoorStatus();
      } catch (error, stackTrace) {
        debugPrint('CircadianExpert: outdoor status load failed: $error');
        debugPrint('$stackTrace');
      }
      if (!mounted) return;
      setState(() {
        _config = config;
        _outdoorStatus = outdoorStatus;
        _loading = false;
      });
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: config load failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() {
        _loading = false;
        _error = 'Settings load failed: $error';
      });
    }
  }

  Future<void> _saveField(String key, Object? value) async {
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before saving expert settings.');
      return;
    }
    if (!_isExpertConfigKeyWritable(key)) {
      _showSnack(
        'This build is missing the Expert settings endpoint for "$key".',
      );
      return;
    }

    final previous = _config;
    HapticFeedback.selectionClick();
    setState(() {
      _config = _config.withField(key, value);
      _saving = true;
      _savingKey = key;
      _error = null;
    });
    try {
      await client.saveExpertConfig({key: value});
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: config save failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() {
        _config = previous;
        _error = 'Settings save failed: $error';
      });
      _showSnack('Settings save failed: $error');
    } finally {
      if (mounted) {
        setState(() {
          _saving = false;
          _savingKey = null;
        });
      }
    }
  }

  Future<void> _loadOutdoorStatus({bool silent = false}) async {
    final client = widget.client;
    if (client == null) return;
    if (!silent) setState(() => _outdoorLoading = true);
    try {
      final status = await client.fetchOutdoorStatus();
      if (!mounted) return;
      setState(() {
        _outdoorStatus = status;
        _outdoorLoading = false;
      });
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: outdoor status refresh failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() => _outdoorLoading = false);
      _showSnack('Outdoor status failed: $error');
    }
  }

  Future<void> _runOutdoorAction({
    required String key,
    required Future<void> Function(_CircadianExpertClient client) action,
    required String success,
  }) async {
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before changing sun intensity.');
      return;
    }
    HapticFeedback.selectionClick();
    setState(() {
      _outdoorSavingKey = key;
      _error = null;
    });
    try {
      await action(client);
      await _loadOutdoorStatus(silent: true);
      _showSnack(success);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: outdoor action failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() => _error = 'Sun intensity action failed: $error');
      _showSnack('Sun intensity action failed: $error');
    } finally {
      if (mounted) setState(() => _outdoorSavingKey = null);
    }
  }

  Future<void> _setDailySync(TimeOfDay time) async {
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before saving expert settings.');
      return;
    }
    final previous = _config;
    HapticFeedback.selectionClick();
    setState(() {
      _config = _config
          .withField('daily_sync_hour', time.hour)
          .withField('daily_sync_minute', time.minute);
      _saving = true;
      _savingKey = 'daily_sync_time';
      _error = null;
    });
    try {
      await client.saveExpertConfig({
        'daily_sync_hour': time.hour,
        'daily_sync_minute': time.minute,
      });
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: daily sync save failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() {
        _config = previous;
        _error = 'Daily sync save failed: $error';
      });
      _showSnack('Daily sync save failed: $error');
    } finally {
      if (mounted) {
        setState(() {
          _saving = false;
          _savingKey = null;
        });
      }
    }
  }

  Future<void> _pickDailySyncTime() async {
    final initial = TimeOfDay(
      hour:
          _config.intValue('daily_sync_hour', fallback: 4).clamp(0, 23).toInt(),
      minute: _config
          .intValue('daily_sync_minute', fallback: 0)
          .clamp(0, 59)
          .toInt(),
    );
    final picked = await showTimePicker(
      context: context,
      initialTime: initial,
      builder: (context, child) => Theme(
        data: Theme.of(context).copyWith(
          colorScheme: const ColorScheme.dark(
            primary: _expertAccent,
            surface: CelestialColors.backgroundCard,
          ),
        ),
        child: child!,
      ),
    );
    if (picked == null) return;
    await _setDailySync(picked);
  }

  Future<void> _syncEndpoint(String endpoint, String label) async {
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before syncing.');
      return;
    }
    HapticFeedback.selectionClick();
    setState(() {
      _saving = true;
      _savingKey = endpoint;
      _error = null;
    });
    try {
      await client.postExpertEndpoint(endpoint);
      _showSnack('$label complete');
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: sync failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() => _error = '$label failed: $error');
    } finally {
      if (mounted) {
        setState(() {
          _saving = false;
          _savingKey = null;
        });
      }
    }
  }

  void _showSnack(String message) {
    if (!mounted) return;
    ScaffoldMessenger.of(context)
        .showSnackBar(SnackBar(content: Text(message)));
  }

  @override
  Widget build(BuildContext context) {
    final pauseDurationValue = _config
        .intValue('default_pause_duration_minutes', fallback: 240)
        .toString();
    final freezeDurationValue = _config
        .intValue('default_freeze_duration_minutes', fallback: 0)
        .toString();
    final boostDurationValue = _config
        .intValue('default_boost_duration_minutes', fallback: 60)
        .toString();
    final powerOffDurationValue = _config
        .intValue('default_power_off_duration_minutes', fallback: 60)
        .toString();
    final durationPresetValues = _durationPresetValues(
      _config,
      includeValues: [
        pauseDurationValue,
        freezeDurationValue,
        boostDurationValue,
        powerOffDurationValue,
      ],
    );
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            _ExpertHeader(
              title: 'Settings',
              leading:
                  _BackButton(onPressed: () => Navigator.of(context).pop()),
              trailing: IconButton(
                onPressed: _loading ? null : _loadConfig,
                tooltip: 'Refresh settings',
                icon: const Icon(Icons.refresh_rounded),
                color: CelestialColors.textSecondary,
                style: IconButton.styleFrom(
                  backgroundColor: _panel2Color,
                  fixedSize: const Size(40, 40),
                  minimumSize: const Size(40, 40),
                  shape: RoundedRectangleBorder(
                    borderRadius: BorderRadius.circular(8),
                  ),
                  side: const BorderSide(color: CelestialColors.orbitRing),
                ),
              ),
            ),
            Expanded(
              child: ListView(
                padding: const EdgeInsets.fromLTRB(16, 0, 16, 28),
                children: [
                  if (_loading || _saving || _error != null)
                    _ExpertPanel(
                      padding: const EdgeInsets.all(12),
                      child: Row(
                        children: [
                          if (_loading || _saving) ...[
                            const SizedBox(
                              width: 18,
                              height: 18,
                              child: CircularProgressIndicator(strokeWidth: 2),
                            ),
                            const SizedBox(width: 10),
                          ],
                          Expanded(
                            child: Text(
                              _error ??
                                  (_saving
                                      ? 'Saving ${_savingKey ?? 'settings'}...'
                                      : 'Loading settings...'),
                              style: TextStyle(
                                color: _error == null
                                    ? CelestialColors.textSecondary
                                    : _dangerAccent,
                                fontSize: 12,
                                fontWeight: FontWeight.w700,
                              ),
                            ),
                          ),
                        ],
                      ),
                    ),
                  if (_loading) const SizedBox(height: 12),
                  _ConfigSectionCard(
                    title: 'Main',
                    children: [
                      _ConfigDropdownRow(
                        label: 'Power fail recovery',
                        value: _config.stringValue(
                          'power_recovery',
                          fallback: 'last_state',
                        ),
                        items: const ['last_state', 'bright'],
                        itemLabel: (value) =>
                            value == 'bright' ? 'Bright' : 'Last state',
                        saving: _savingKey == 'power_recovery',
                        onChanged: (value) => unawaited(
                          _saveField('power_recovery', value),
                        ),
                      ),
                      _ConfigTimeRow(
                        label: 'Daily sync',
                        hour: _config.intValue('daily_sync_hour', fallback: 4),
                        minute:
                            _config.intValue('daily_sync_minute', fallback: 0),
                        saving: _savingKey == 'daily_sync_time',
                        onTap: _pickDailySyncTime,
                      ),
                      Wrap(
                        spacing: 8,
                        runSpacing: 8,
                        children: [
                          OutlinedButton.icon(
                            onPressed: _saving
                                ? null
                                : () => unawaited(
                                      _syncEndpoint(
                                        'api/integrations/sync-controls',
                                        'Sync controls',
                                      ),
                                    ),
                            icon: const Icon(Icons.sensors_rounded, size: 17),
                            label: const Text('Sync controls'),
                            style: OutlinedButton.styleFrom(
                              foregroundColor: _expertAccent,
                              side: const BorderSide(
                                  color: CelestialColors.orbitRing),
                              shape: RoundedRectangleBorder(
                                borderRadius: BorderRadius.circular(8),
                              ),
                            ),
                          ),
                          OutlinedButton.icon(
                            onPressed: _saving
                                ? null
                                : () => unawaited(
                                      _syncEndpoint(
                                        'api/integrations/sync-devices',
                                        'Sync lights',
                                      ),
                                    ),
                            icon: const Icon(Icons.lightbulb_rounded, size: 17),
                            label: const Text('Sync lights'),
                            style: OutlinedButton.styleFrom(
                              foregroundColor: _expertAccent,
                              side: const BorderSide(
                                  color: CelestialColors.orbitRing),
                              shape: RoundedRectangleBorder(
                                borderRadius: BorderRadius.circular(8),
                              ),
                            ),
                          ),
                        ],
                      ),
                      _OutdoorStatusPanel(
                        status: _outdoorStatus,
                        loading: _outdoorLoading,
                        savingKey: _outdoorSavingKey,
                        onRefresh: () => unawaited(
                          _runOutdoorAction(
                            key: 'refresh',
                            action: (client) => client.refreshOutdoor(),
                            success: 'Sun intensity refreshed',
                          ),
                        ),
                        onLearnBaselines: () => unawaited(
                          _runOutdoorAction(
                            key: 'learn',
                            action: (client) => client.learnBaselines(),
                            success: 'Baselines learned',
                          ),
                        ),
                        onSetOverride: (condition, durationMinutes) =>
                            unawaited(
                          _runOutdoorAction(
                            key: 'override',
                            action: (client) => client.setOutdoorOverride(
                              condition: condition,
                              durationMinutes: durationMinutes,
                            ),
                            success: 'Sky clarity override set',
                          ),
                        ),
                        onClearOverride: () => unawaited(
                          _runOutdoorAction(
                            key: 'clear_override',
                            action: (client) => client.clearOutdoorOverride(),
                            success: 'Sky clarity override cleared',
                          ),
                        ),
                      ),
                    ],
                  ),
                  _ConfigSectionCard(
                    title: 'Light Behavior',
                    children: [
                      _ConfigNumberRow(
                        label: 'Power on speed',
                        unit: 'tenths',
                        value: _config.doubleValue(
                          'turn_on_transition',
                          fallback: 3,
                        ),
                        min: 0,
                        max: 100,
                        step: 0.1,
                        saving: _savingKey == 'turn_on_transition',
                        onChanged: (value) => unawaited(
                          _saveField('turn_on_transition', value),
                        ),
                      ),
                      _ConfigNumberRow(
                        label: 'Power off speed',
                        unit: 'tenths',
                        value: _config.doubleValue(
                          'turn_off_transition',
                          fallback: 3,
                        ),
                        min: 0,
                        max: 100,
                        step: 0.1,
                        saving: _savingKey == 'turn_off_transition',
                        onChanged: (value) => unawaited(
                          _saveField('turn_off_transition', value),
                        ),
                      ),
                      _ConfigNumberRow(
                        label: 'Boost return speed',
                        unit: 'tenths',
                        value: _config.doubleValue(
                          'boost_return_transition',
                          fallback: 60,
                        ),
                        min: 0,
                        max: 100,
                        step: 1,
                        saving: _savingKey == 'boost_return_transition',
                        enabled: _isExpertConfigKeyWritable(
                            'boost_return_transition'),
                        onChanged: (value) => unawaited(
                          _saveField('boost_return_transition', value),
                        ),
                      ),
                      _ConfigToggleRow(
                        label: 'Feedback one area only',
                        value: _config.boolValue(
                          'feedback_restrict_to_primary',
                          fallback: false,
                        ),
                        saving: _savingKey == 'feedback_restrict_to_primary',
                        enabled: _isExpertConfigKeyWritable(
                          'feedback_restrict_to_primary',
                        ),
                        onChanged: (value) => unawaited(
                          _saveField('feedback_restrict_to_primary', value),
                        ),
                      ),
                      _ConfigToggleRow(
                        label: 'Reach feedback',
                        value: _config.boolValue(
                          'reach_feedback_enabled',
                          fallback: true,
                        ),
                        saving: _savingKey == 'reach_feedback_enabled',
                        enabled: _isExpertConfigKeyWritable(
                            'reach_feedback_enabled'),
                        onChanged: (value) => unawaited(
                          _saveField('reach_feedback_enabled', value),
                        ),
                      ),
                      _ConfigNumberRow(
                        label: 'Reach daytime threshold',
                        unit: '%',
                        value: _config.intValue(
                          'reach_daytime_threshold',
                          fallback: 50,
                        ),
                        min: 5,
                        max: 80,
                        step: 5,
                        saving: _savingKey == 'reach_daytime_threshold',
                        enabled: _isExpertConfigKeyWritable(
                          'reach_daytime_threshold',
                        ),
                        onChanged: (value) => unawaited(
                          _saveField('reach_daytime_threshold', value),
                        ),
                      ),
                      _ConfigToggleRow(
                        label: 'Freeze feedback',
                        value: _config.boolValue(
                          'freeze_feedback_enabled',
                          fallback: true,
                        ),
                        saving: _savingKey == 'freeze_feedback_enabled',
                        enabled: _isExpertConfigKeyWritable(
                          'freeze_feedback_enabled',
                        ),
                        onChanged: (value) => unawaited(
                          _saveField('freeze_feedback_enabled', value),
                        ),
                      ),
                      _ConfigNumberRow(
                        label: 'Freeze hold',
                        unit: 'tenths',
                        value: _config.intValue(
                          'freeze_hold_at_dim',
                          fallback: 2,
                        ),
                        min: 0,
                        max: 50,
                        step: 1,
                        saving: _savingKey == 'freeze_hold_at_dim',
                        enabled:
                            _isExpertConfigKeyWritable('freeze_hold_at_dim'),
                        onChanged: (value) => unawaited(
                          _saveField('freeze_hold_at_dim', value),
                        ),
                      ),
                      _ConfigNumberRow(
                        label: 'Freeze off rise',
                        unit: 'tenths',
                        value: _config.doubleValue(
                          'freeze_off_rise',
                          fallback: 10,
                        ),
                        min: 0,
                        max: 100,
                        step: 0.5,
                        saving: _savingKey == 'freeze_off_rise',
                        enabled: _isExpertConfigKeyWritable('freeze_off_rise'),
                        onChanged: (value) => unawaited(
                          _saveField('freeze_off_rise', value),
                        ),
                      ),
                      _ConfigToggleRow(
                        label: 'Bounce feedback',
                        value: _config.boolValue(
                          'limit_bounce_enabled',
                          fallback: true,
                        ),
                        saving: _savingKey == 'limit_bounce_enabled',
                        enabled:
                            _isExpertConfigKeyWritable('limit_bounce_enabled'),
                        onChanged: (value) => unawaited(
                          _saveField('limit_bounce_enabled', value),
                        ),
                      ),
                      _ConfigNumberRow(
                        label: 'Small bounce max',
                        unit: '%',
                        value: _config.intValue(
                          'limit_bounce_max_percent',
                          fallback: 25,
                        ),
                        min: 5,
                        max: 50,
                        step: 1,
                        saving: _savingKey == 'limit_bounce_max_percent',
                        enabled: _isExpertConfigKeyWritable(
                          'limit_bounce_max_percent',
                        ),
                        onChanged: (value) => unawaited(
                          _saveField('limit_bounce_max_percent', value),
                        ),
                      ),
                      _ConfigNumberRow(
                        label: 'Small bounce min',
                        unit: '%',
                        value: _config.intValue(
                          'limit_bounce_min_percent',
                          fallback: 13,
                        ),
                        min: 5,
                        max: 50,
                        step: 1,
                        saving: _savingKey == 'limit_bounce_min_percent',
                        enabled: _isExpertConfigKeyWritable(
                          'limit_bounce_min_percent',
                        ),
                        onChanged: (value) => unawaited(
                          _saveField('limit_bounce_min_percent', value),
                        ),
                      ),
                      _ConfigNumberRow(
                        label: 'Limit bounce speed',
                        unit: 'tenths',
                        value: _config.doubleValue(
                          'limit_warning_speed',
                          fallback: 3,
                        ),
                        min: 0,
                        max: 100,
                        step: 0.5,
                        saving: _savingKey == 'limit_warning_speed',
                        enabled:
                            _isExpertConfigKeyWritable('limit_warning_speed'),
                        onChanged: (value) => unawaited(
                          _saveField('limit_warning_speed', value),
                        ),
                      ),
                      _ConfigNumberRow(
                        label: 'Alert bounce speed',
                        unit: 'tenths',
                        value: _config.intValue(
                          'alert_bounce_speed',
                          fallback: 10,
                        ),
                        min: 1,
                        max: 50,
                        step: 1,
                        saving: _savingKey == 'alert_bounce_speed',
                        enabled:
                            _isExpertConfigKeyWritable('alert_bounce_speed'),
                        onChanged: (value) => unawaited(
                          _saveField('alert_bounce_speed', value),
                        ),
                      ),
                    ],
                  ),
                  _ConfigSectionCard(
                    title: 'Controls',
                    children: [
                      _ConfigNumberRow(
                        label: 'Long-press repeat',
                        unit: 'tenths',
                        value: _config.doubleValue(
                          'long_press_repeat_interval',
                          fallback: 7,
                        ),
                        min: 1,
                        max: 100,
                        step: 0.5,
                        saving: _savingKey == 'long_press_repeat_interval',
                        onChanged: (value) => unawaited(
                          _saveField('long_press_repeat_interval', value),
                        ),
                      ),
                      _ConfigNumberRow(
                        label: 'Step increments',
                        unit: 'steps',
                        value: _config.intValue('max_dim_steps', fallback: 10),
                        min: 1,
                        max: 50,
                        step: 1,
                        saving: _savingKey == 'max_dim_steps',
                        onChanged: (value) => unawaited(
                          _saveField('max_dim_steps', value),
                        ),
                      ),
                      _ConfigNumberRow(
                        label: 'Step fallback',
                        unit: 'min',
                        value: _config.intValue(
                          'step_fallback_minutes',
                          fallback: 30,
                        ),
                        min: 1,
                        max: 180,
                        step: 1,
                        saving: _savingKey == 'step_fallback_minutes',
                        enabled:
                            _isExpertConfigKeyWritable('step_fallback_minutes'),
                        onChanged: (value) => unawaited(
                          _saveField('step_fallback_minutes', value),
                        ),
                      ),
                      _ConfigNumberRow(
                        label: 'Cursor step',
                        unit: 'min',
                        value: _config.intValue(
                          'rhythm_cursor_step_min',
                          fallback: 5,
                        ),
                        min: 1,
                        max: 60,
                        step: 1,
                        saving: _savingKey == 'rhythm_cursor_step_min',
                        enabled: _isExpertConfigKeyWritable(
                            'rhythm_cursor_step_min'),
                        onChanged: (value) => unawaited(
                          _saveField('rhythm_cursor_step_min', value),
                        ),
                      ),
                      _ConfigNumberRow(
                        label: 'Pulse window',
                        unit: 'h',
                        value: _config.intValue(
                          'controls_pulse_window_hours',
                          fallback: 6,
                        ),
                        min: 1,
                        max: 168,
                        step: 1,
                        saving: _savingKey == 'controls_pulse_window_hours',
                        enabled: _isExpertConfigKeyWritable(
                          'controls_pulse_window_hours',
                        ),
                        onChanged: (value) => unawaited(
                          _saveField('controls_pulse_window_hours', value),
                        ),
                      ),
                      _ConfigNumberRow(
                        label: 'Recent window',
                        unit: 'min',
                        value: _config.intValue(
                          'controls_recent_window_minutes',
                          fallback: 5,
                        ),
                        min: 1,
                        max: 1440,
                        step: 1,
                        saving: _savingKey == 'controls_recent_window_minutes',
                        enabled: _isExpertConfigKeyWritable(
                          'controls_recent_window_minutes',
                        ),
                        onChanged: (value) => unawaited(
                          _saveField('controls_recent_window_minutes', value),
                        ),
                      ),
                      _ConfigNumberRow(
                        label: 'Activity entries',
                        unit: 'entries',
                        value: _config.intValue(
                          'activity_log_min_entries',
                          fallback: 100,
                        ),
                        min: 50,
                        max: 500,
                        step: 10,
                        saving: _savingKey == 'activity_log_min_entries',
                        enabled: _isExpertConfigKeyWritable(
                          'activity_log_min_entries',
                        ),
                        onChanged: (value) => unawaited(
                          _saveField('activity_log_min_entries', value),
                        ),
                      ),
                      _ConfigNumberRow(
                        label: 'Activity days',
                        unit: 'days',
                        value: _config.intValue(
                          'activity_log_min_days',
                          fallback: 7,
                        ),
                        min: 1,
                        max: 30,
                        step: 1,
                        saving: _savingKey == 'activity_log_min_days',
                        enabled: _isExpertConfigKeyWritable(
                          'activity_log_min_days',
                        ),
                        onChanged: (value) => unawaited(
                          _saveField('activity_log_min_days', value),
                        ),
                      ),
                      _ConfigNumberRow(
                        label: 'Activity flush',
                        unit: 'min',
                        value: _config.intValue(
                          'activity_log_flush_interval_minutes',
                          fallback: 15,
                        ),
                        min: 0,
                        max: 1440,
                        step: 15,
                        saving:
                            _savingKey == 'activity_log_flush_interval_minutes',
                        enabled: _isExpertConfigKeyWritable(
                          'activity_log_flush_interval_minutes',
                        ),
                        onChanged: (value) => unawaited(
                          _saveField(
                            'activity_log_flush_interval_minutes',
                            value,
                          ),
                        ),
                      ),
                      _ConfigNumberRow(
                        label: 'Tick retry user',
                        unit: 'ticks',
                        value: _config.intValue(
                          'tick_repeat_after_user_action',
                          fallback: 10,
                        ),
                        min: 0,
                        max: 60,
                        step: 1,
                        saving: _savingKey == 'tick_repeat_after_user_action',
                        enabled: _isExpertConfigKeyWritable(
                          'tick_repeat_after_user_action',
                        ),
                        onChanged: (value) => unawaited(
                          _saveField('tick_repeat_after_user_action', value),
                        ),
                      ),
                      _ConfigNumberRow(
                        label: 'Tick retry auto',
                        unit: 'ticks',
                        value: _config.intValue(
                          'tick_repeat_after_autonomous_change',
                          fallback: 3,
                        ),
                        min: 0,
                        max: 60,
                        step: 1,
                        saving:
                            _savingKey == 'tick_repeat_after_autonomous_change',
                        enabled: _isExpertConfigKeyWritable(
                          'tick_repeat_after_autonomous_change',
                        ),
                        onChanged: (value) => unawaited(
                          _saveField(
                            'tick_repeat_after_autonomous_change',
                            value,
                          ),
                        ),
                      ),
                      _ConfigNumberRow(
                        label: 'Periodic refresh',
                        unit: 'min',
                        value: _config.intValue(
                          'periodic_refresh_interval_minutes',
                          fallback: 15,
                        ),
                        min: 0,
                        max: 1440,
                        step: 15,
                        saving:
                            _savingKey == 'periodic_refresh_interval_minutes',
                        enabled: _isExpertConfigKeyWritable(
                          'periodic_refresh_interval_minutes',
                        ),
                        onChanged: (value) => unawaited(
                          _saveField(
                            'periodic_refresh_interval_minutes',
                            value,
                          ),
                        ),
                      ),
                      _ConfigNumberRow(
                        label: 'Motion warning',
                        unit: 's',
                        value: _config.intValue(
                          'motion_warning_time',
                          fallback: 20,
                        ),
                        min: 0,
                        max: 120,
                        step: 5,
                        saving: _savingKey == 'motion_warning_time',
                        onChanged: (value) => unawaited(
                          _saveField('motion_warning_time', value),
                        ),
                      ),
                      _ConfigNumberRow(
                        label: 'Motion blink threshold',
                        unit: '%',
                        value: _config.intValue(
                          'motion_blink_threshold',
                          fallback: 15,
                        ),
                        min: 5,
                        max: 50,
                        step: 1,
                        saving: _savingKey == 'motion_blink_threshold',
                        onChanged: (value) => unawaited(
                          _saveField('motion_blink_threshold', value),
                        ),
                      ),
                      _ConfigToggleRow(
                        label: 'Multi-click',
                        value: _config.boolValue(
                          'multi_click_enabled',
                          fallback: true,
                        ),
                        saving: _savingKey == 'multi_click_enabled',
                        onChanged: (value) => unawaited(
                          _saveField('multi_click_enabled', value),
                        ),
                      ),
                      _ConfigNumberRow(
                        label: 'Multi-click speed',
                        unit: 's',
                        value: _config.doubleValue(
                          'multi_click_speed',
                          fallback: 1.5,
                        ),
                        min: 0.1,
                        max: 10,
                        step: 0.1,
                        saving: _savingKey == 'multi_click_speed',
                        onChanged: (value) => unawaited(
                          _saveField('multi_click_speed', value),
                        ),
                      ),
                    ],
                  ),
                  _ConfigSectionCard(
                    title: 'Quirky Hardware',
                    children: [
                      _ConfigNumberRow(
                        label: '2-step brightness delta',
                        unit: '%',
                        value: _config.intValue(
                          'two_step_bri_threshold',
                          fallback: 15,
                        ),
                        min: 0,
                        max: 50,
                        step: 1,
                        saving: _savingKey == 'two_step_bri_threshold',
                        onChanged: (value) => unawaited(
                          _saveField('two_step_bri_threshold', value),
                        ),
                      ),
                      _ConfigNumberRow(
                        label: '2-step delay',
                        unit: 'tenths',
                        value: _config.doubleValue(
                          'two_step_delay',
                          fallback: 5,
                        ),
                        min: 0,
                        max: 20,
                        step: 0.5,
                        saving: _savingKey == 'two_step_delay',
                        onChanged: (value) => unawaited(
                          _saveField('two_step_delay', value),
                        ),
                      ),
                      _ConfigToggleRow(
                        label: 'CT compensation',
                        value: _config.boolValue(
                          'ct_comp_enabled',
                          fallback: false,
                        ),
                        saving: _savingKey == 'ct_comp_enabled',
                        onChanged: (value) => unawaited(
                          _saveField('ct_comp_enabled', value),
                        ),
                      ),
                      _ConfigNumberRow(
                        label: 'CT handover begin',
                        unit: 'K',
                        value:
                            _config.intValue('ct_comp_begin', fallback: 1650),
                        min: 500,
                        max: 2500,
                        step: 50,
                        saving: _savingKey == 'ct_comp_begin',
                        onChanged: (value) => unawaited(
                          _saveField('ct_comp_begin', value),
                        ),
                      ),
                      _ConfigNumberRow(
                        label: 'CT handover end',
                        unit: 'K',
                        value: _config.intValue('ct_comp_end', fallback: 2250),
                        min: 1500,
                        max: 3500,
                        step: 50,
                        saving: _savingKey == 'ct_comp_end',
                        onChanged: (value) => unawaited(
                          _saveField('ct_comp_end', value),
                        ),
                      ),
                      _ConfigNumberRow(
                        label: 'CT max compensation',
                        unit: 'x',
                        value: _config.doubleValue(
                          'ct_comp_factor',
                          fallback: 1.7,
                        ),
                        min: 1,
                        max: 2,
                        step: 0.05,
                        saving: _savingKey == 'ct_comp_factor',
                        onChanged: (value) => unawaited(
                          _saveField('ct_comp_factor', value),
                        ),
                      ),
                      _ConfigNumberRow(
                        label: 'Sun saturation',
                        unit: '%',
                        value: _config.intValue('sun_saturation', fallback: 40),
                        min: 1,
                        max: 100,
                        step: 5,
                        saving: _savingKey == 'sun_saturation',
                        enabled: _isExpertConfigKeyWritable('sun_saturation'),
                        onChanged: (value) => unawaited(
                          _saveField('sun_saturation', value),
                        ),
                      ),
                    ],
                  ),
                  _ConfigSectionCard(
                    title: 'Light Lab',
                    children: [
                      _ConfigDropdownRow(
                        label: 'Default pause',
                        value: pauseDurationValue,
                        items: durationPresetValues,
                        itemLabel: _durationOptionLabel,
                        saving: _savingKey == 'default_pause_duration_minutes',
                        enabled: _isExpertConfigKeyWritable(
                          'default_pause_duration_minutes',
                        ),
                        onChanged: (value) => unawaited(
                          _saveField(
                            'default_pause_duration_minutes',
                            int.parse(value),
                          ),
                        ),
                      ),
                      _ConfigDropdownRow(
                        label: 'Default freeze',
                        value: freezeDurationValue,
                        items: durationPresetValues,
                        itemLabel: _durationOptionLabel,
                        saving: _savingKey == 'default_freeze_duration_minutes',
                        enabled: _isExpertConfigKeyWritable(
                          'default_freeze_duration_minutes',
                        ),
                        onChanged: (value) => unawaited(
                          _saveField(
                            'default_freeze_duration_minutes',
                            int.parse(value),
                          ),
                        ),
                      ),
                      _ConfigDropdownRow(
                        label: 'Default boost',
                        value: boostDurationValue,
                        items: durationPresetValues,
                        itemLabel: _durationOptionLabel,
                        saving: _savingKey == 'default_boost_duration_minutes',
                        enabled: _isExpertConfigKeyWritable(
                          'default_boost_duration_minutes',
                        ),
                        onChanged: (value) => unawaited(
                          _saveField(
                            'default_boost_duration_minutes',
                            int.parse(value),
                          ),
                        ),
                      ),
                      _ConfigNumberRow(
                        label: 'Boost intensity',
                        unit: '%',
                        value: _config.intValue('boost_default', fallback: 30),
                        min: 10,
                        max: 100,
                        step: 5,
                        saving: _savingKey == 'boost_default',
                        enabled: _isExpertConfigKeyWritable('boost_default'),
                        onChanged: (value) => unawaited(
                          _saveField('boost_default', value),
                        ),
                      ),
                      _ConfigToggleRow(
                        label: 'Confirm zone pushes',
                        value: _config.boolValue(
                          'confirm_zone_pushes',
                          fallback: true,
                        ),
                        saving: _savingKey == 'confirm_zone_pushes',
                        enabled: _isExpertConfigKeyWritable(
                          'confirm_zone_pushes',
                        ),
                        onChanged: (value) => unawaited(
                          _saveField('confirm_zone_pushes', value),
                        ),
                      ),
                      _ConfigDropdownRow(
                        label: 'Default power-off',
                        value: powerOffDurationValue,
                        items: durationPresetValues,
                        itemLabel: _durationOptionLabel,
                        saving:
                            _savingKey == 'default_power_off_duration_minutes',
                        enabled: _isExpertConfigKeyWritable(
                          'default_power_off_duration_minutes',
                        ),
                        onChanged: (value) => unawaited(
                          _saveField(
                            'default_power_off_duration_minutes',
                            int.parse(value),
                          ),
                        ),
                      ),
                      _ConfigToggleRow(
                        label: 'Read-only ZHA',
                        value: _config.boolValue(
                          'read_only_zha',
                          fallback: false,
                        ),
                        saving: _savingKey == 'read_only_zha',
                        onChanged: (value) => unawaited(
                          _saveField('read_only_zha', value),
                        ),
                      ),
                    ],
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _ExpertMomentsScreen extends StatefulWidget {
  final _CircadianExpertClient? client;
  final List<_ExpertZone> zones;
  final bool showBackButton;

  const _ExpertMomentsScreen({
    required this.client,
    required this.zones,
    this.showBackButton = true,
  });

  @override
  State<_ExpertMomentsScreen> createState() => _ExpertMomentsScreenState();
}

class _ExpertMomentsScreenState extends State<_ExpertMomentsScreen> {
  _ExpertMomentsLoad _load = const _ExpertMomentsLoad.empty();
  bool _loading = true;
  String? _busyMomentId;
  String? _error;
  String _sortMode = 'name';

  @override
  void initState() {
    super.initState();
    unawaited(_loadMoments());
  }

  Future<void> _loadMoments() async {
    final client = widget.client;
    if (client == null) {
      setState(() {
        _loading = false;
        _error = 'Connect to RhythmOS to manage moments.';
      });
      return;
    }

    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final load = await client.fetchMoments();
      if (!mounted) return;
      setState(() {
        _load = load;
        _loading = false;
      });
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: moments load failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() {
        _loading = false;
        _error = 'Moments load failed: $error';
      });
    }
  }

  Future<void> _createMoment() async {
    final name = await _promptMomentName(title: 'New moment');
    if (name == null) return;
    final client = widget.client;
    if (client == null) return;

    HapticFeedback.selectionClick();
    setState(() => _busyMomentId = '__create__');
    try {
      await client.createMoment(name);
      await _loadMoments();
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: moment create failed: $error');
      debugPrint('$stackTrace');
      _showSnack('Moment create failed: $error');
    } finally {
      if (mounted) setState(() => _busyMomentId = null);
    }
  }

  Future<void> _runMoment(_ExpertMoment moment) async {
    final client = widget.client;
    if (client == null) return;
    final confirmed = await _confirm(
      title: 'Apply moment',
      message: 'Apply "${moment.name}" now?',
      confirm: 'Apply',
    );
    if (!confirmed) return;

    HapticFeedback.selectionClick();
    setState(() => _busyMomentId = moment.id);
    try {
      await client.runMoment(moment.id);
      _showSnack('Applied ${moment.name}');
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: moment run failed: $error');
      debugPrint('$stackTrace');
      _showSnack('Moment run failed: $error');
    } finally {
      if (mounted) setState(() => _busyMomentId = null);
    }
  }

  Future<void> _deleteMoment(_ExpertMoment moment) async {
    final client = widget.client;
    if (client == null) return;
    final confirmed = await _confirm(
      title: 'Delete moment',
      message: 'Delete "${moment.name}"?',
      confirm: 'Delete',
      danger: true,
    );
    if (!confirmed) return;

    HapticFeedback.selectionClick();
    setState(() => _busyMomentId = moment.id);
    try {
      await client.deleteMoment(moment.id);
      await _loadMoments();
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: moment delete failed: $error');
      debugPrint('$stackTrace');
      _showSnack('Moment delete failed: $error');
    } finally {
      if (mounted) setState(() => _busyMomentId = null);
    }
  }

  Future<void> _openMoment(_ExpertMoment moment) async {
    await Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) => _ExpertMomentDetailScreen(
          client: widget.client,
          initialMoment: moment,
          zones: widget.zones,
        ),
      ),
    );
    if (mounted) unawaited(_loadMoments());
  }

  List<_ExpertMoment> _sortedMoments() {
    final moments = List<_ExpertMoment>.from(_load.moments);
    moments.sort((a, b) {
      final compare = switch (_sortMode) {
        'usage' => b.usageCount.compareTo(a.usageCount),
        'action' => _momentActionLabel(a.defaultAction)
            .compareTo(_momentActionLabel(b.defaultAction)),
        _ => a.name.toLowerCase().compareTo(b.name.toLowerCase()),
      };
      return compare != 0
          ? compare
          : a.name.toLowerCase().compareTo(b.name.toLowerCase());
    });
    return moments;
  }

  Future<String?> _promptMomentName({
    required String title,
    String initialValue = '',
  }) async {
    final controller = TextEditingController(text: initialValue);
    final value = await showDialog<String>(
      context: context,
      builder: (context) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: Text(
          title,
          style: const TextStyle(color: CelestialColors.textPrimary),
        ),
        content: TextField(
          controller: controller,
          autofocus: true,
          style: const TextStyle(color: CelestialColors.textPrimary),
          decoration: const InputDecoration(
            labelText: 'Moment name',
            labelStyle: TextStyle(color: CelestialColors.textSecondary),
            enabledBorder: UnderlineInputBorder(
              borderSide: BorderSide(color: CelestialColors.orbitRing),
            ),
            focusedBorder: UnderlineInputBorder(
              borderSide: BorderSide(color: _expertAccent),
            ),
          ),
          onSubmitted: (value) => Navigator.of(context).pop(value),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(),
            child: const Text('Cancel'),
          ),
          TextButton(
            onPressed: () => Navigator.of(context).pop(controller.text),
            child: const Text('Create'),
          ),
        ],
      ),
    );
    controller.dispose();
    final trimmed = value?.trim();
    return trimmed == null || trimmed.isEmpty ? null : trimmed;
  }

  Future<bool> _confirm({
    required String title,
    required String message,
    required String confirm,
    bool danger = false,
  }) async {
    final result = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: Text(
          title,
          style: const TextStyle(color: CelestialColors.textPrimary),
        ),
        content: Text(
          message,
          style: const TextStyle(color: CelestialColors.textSecondary),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('Cancel'),
          ),
          TextButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: Text(
              confirm,
              style: TextStyle(color: danger ? _dangerAccent : _expertAccent),
            ),
          ),
        ],
      ),
    );
    return result == true;
  }

  void _showSnack(String message) {
    if (!mounted) return;
    ScaffoldMessenger.of(context)
        .showSnackBar(SnackBar(content: Text(message)));
  }

  @override
  Widget build(BuildContext context) {
    final moments = _sortedMoments();
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            _ExpertHeader(
              title: 'Moments',
              leading: widget.showBackButton
                  ? _BackButton(onPressed: () => Navigator.of(context).pop())
                  : const _ExpertModeBadge(),
              trailing: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  IconButton(
                    onPressed: _loading ? null : _loadMoments,
                    tooltip: 'Refresh moments',
                    icon: const Icon(Icons.refresh_rounded),
                    color: CelestialColors.textSecondary,
                    visualDensity: VisualDensity.compact,
                  ),
                  IconButton(
                    onPressed: _busyMomentId == null ? _createMoment : null,
                    tooltip: 'New moment',
                    icon: const Icon(Icons.add_rounded),
                    color: _expertAccent,
                    visualDensity: VisualDensity.compact,
                  ),
                ],
              ),
            ),
            Expanded(
              child: ListView(
                padding: const EdgeInsets.fromLTRB(16, 0, 16, 28),
                children: [
                  _ExpertPanel(
                    padding: const EdgeInsets.all(12),
                    child: Row(
                      children: [
                        Expanded(
                          child: _MomentSortChips(
                            value: _sortMode,
                            onChanged: (value) =>
                                setState(() => _sortMode = value),
                          ),
                        ),
                        if (_loading || _busyMomentId == '__create__') ...[
                          const SizedBox(width: 10),
                          const SizedBox(
                            width: 18,
                            height: 18,
                            child: CircularProgressIndicator(strokeWidth: 2),
                          ),
                        ],
                      ],
                    ),
                  ),
                  if (_error != null) ...[
                    const SizedBox(height: 10),
                    _ExpertPanel(
                      child: Text(
                        _error!,
                        style: const TextStyle(
                          color: _dangerAccent,
                          fontSize: 12,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                    ),
                  ],
                  const SizedBox(height: 12),
                  if (!_loading && moments.isEmpty)
                    const _ExpertPanel(
                      child: Text(
                        'No moments yet.',
                        style: TextStyle(
                          color: CelestialColors.textSecondary,
                          fontSize: 13,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                    )
                  else
                    for (final moment in moments)
                      _MomentRow(
                        moment: moment,
                        busy: _busyMomentId == moment.id,
                        onOpen: () => _openMoment(moment),
                        onRun: () => _runMoment(moment),
                        onDelete: () => _deleteMoment(moment),
                      ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _ExpertMomentDetailScreen extends StatefulWidget {
  final _CircadianExpertClient? client;
  final _ExpertMoment initialMoment;
  final List<_ExpertZone> zones;

  const _ExpertMomentDetailScreen({
    required this.client,
    required this.initialMoment,
    required this.zones,
  });

  @override
  State<_ExpertMomentDetailScreen> createState() =>
      _ExpertMomentDetailScreenState();
}

class _ExpertMomentDetailScreenState extends State<_ExpertMomentDetailScreen> {
  late _ExpertMoment _moment = widget.initialMoment;
  late final TextEditingController _nameController =
      TextEditingController(text: widget.initialMoment.name);
  _ExpertSwitchesLoad _switchesLoad = const _ExpertSwitchesLoad.empty();
  bool _loading = true;
  bool _saving = false;
  bool _running = false;
  String? _error;
  String? _assignmentBusyKey;
  String _newExceptionAction = 'leave_alone';
  int _newExceptionMinutes = 0;
  String? _newExceptionAreaId;

  @override
  void initState() {
    super.initState();
    unawaited(_loadMoment());
  }

  @override
  void dispose() {
    _nameController.dispose();
    super.dispose();
  }

  List<_ExpertArea> get _areas => [
        for (final zone in widget.zones) ...zone.areas,
      ];

  List<_ExpertMomentAssignment> get _assignments {
    return _momentAssignmentsFor(_moment.id, _switchesLoad.controls);
  }

  Future<void> _loadMoment() async {
    final client = widget.client;
    if (client == null) {
      setState(() {
        _loading = false;
        _error = 'Connect to RhythmOS to edit this moment.';
      });
      return;
    }

    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final results = await Future.wait<Object?>([
        client.fetchMoment(
          widget.initialMoment.id,
          usageCount: _moment.usageCount,
        ),
        client.fetchSwitches().catchError((
          Object error,
          StackTrace stackTrace,
        ) {
          debugPrint('CircadianExpert: moment controls load failed: $error');
          debugPrint('$stackTrace');
          return const _ExpertSwitchesLoad.empty();
        }),
      ]);
      if (!mounted) return;
      setState(() {
        final moment = results[0] as _ExpertMoment;
        final switchesLoad = results[1] as _ExpertSwitchesLoad;
        final assignments =
            _momentAssignmentsFor(moment.id, switchesLoad.controls);
        _switchesLoad = switchesLoad;
        _moment = moment.copyWith(usageCount: assignments.length);
        _nameController.text = _moment.name;
        _loading = false;
      });
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: moment detail load failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() {
        _loading = false;
        _error = 'Moment load failed: $error';
      });
    }
  }

  Future<void> _runMoment() async {
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before applying this moment.');
      return;
    }

    HapticFeedback.selectionClick();
    setState(() => _running = true);
    try {
      await client.runMoment(_moment.id);
      _showSnack('Applied ${_moment.name}');
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: moment detail run failed: $error');
      debugPrint('$stackTrace');
      _showSnack('Moment run failed: $error');
    } finally {
      if (mounted) setState(() => _running = false);
    }
  }

  Future<void> _saveName() async {
    final nextName = _nameController.text.trim();
    if (nextName.isEmpty) {
      _nameController.text = _moment.name;
      return;
    }
    if (nextName == _moment.name) return;
    await _saveFields(
      _moment.copyWith(name: nextName),
      {'name': nextName},
    );
  }

  Future<void> _setIcon(String icon) async {
    if (icon == _moment.icon) return;
    await _saveFields(_moment.copyWith(icon: icon), {'icon': icon});
  }

  Future<void> _setCategory(String category) async {
    if (category == _moment.category) return;
    await _saveFields(
      _moment.copyWith(category: category),
      {'category': category},
    );
  }

  Future<void> _setDefaultAction(String action) async {
    if (action == _moment.defaultAction) return;
    await _saveFields(
      _moment.copyWith(defaultAction: action),
      {'default_action': action},
    );
  }

  Future<void> _setTimerMinutes(int minutes) async {
    final safeSeconds = minutes.clamp(0, 480).toInt() * 60;
    if (safeSeconds == _moment.timerSeconds) return;
    await _saveFields(
      _moment.copyWith(timerSeconds: safeSeconds),
      {'timer': safeSeconds},
    );
  }

  Future<void> _saveExceptions(
    Map<String, _ExpertMomentException> exceptions,
  ) async {
    await _saveFields(
      _moment.copyWith(exceptions: exceptions),
      {'exceptions': _momentExceptionsToServer(exceptions)},
    );
  }

  Future<void> _saveFields(
    _ExpertMoment next,
    Map<String, Object?> fields,
  ) async {
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before saving this moment.');
      return;
    }

    final previous = _moment;
    setState(() {
      _moment = next;
      _saving = true;
      _error = null;
    });
    try {
      await client.updateMomentFields(_moment.id, fields);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: moment save failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() {
        _moment = previous;
        _nameController.text = previous.name;
        _error = 'Moment save failed: $error';
      });
      _showSnack('Moment save failed: $error');
    } finally {
      if (mounted) setState(() => _saving = false);
    }
  }

  void _addException() {
    final availableIds = _availableExceptionAreaIds();
    if (availableIds.isEmpty) return;
    final areaId = availableIds.contains(_newExceptionAreaId)
        ? _newExceptionAreaId!
        : availableIds.first;
    final next = Map<String, _ExpertMomentException>.from(_moment.exceptions);
    next[areaId] = _ExpertMomentException(
      action: _newExceptionAction,
      timerSeconds: _newExceptionMinutes.clamp(0, 480).toInt() * 60,
    );
    setState(() => _newExceptionAreaId = null);
    unawaited(_saveExceptions(next));
  }

  void _updateExceptionAction(String areaId, String action) {
    final current = _moment.exceptions[areaId];
    if (current == null || current.action == action) return;
    final next = Map<String, _ExpertMomentException>.from(_moment.exceptions);
    next[areaId] = current.copyWith(action: action);
    unawaited(_saveExceptions(next));
  }

  void _updateExceptionMinutes(String areaId, int minutes) {
    final current = _moment.exceptions[areaId];
    if (current == null) return;
    final seconds = minutes.clamp(0, 480).toInt() * 60;
    if (current.timerSeconds == seconds) return;
    final next = Map<String, _ExpertMomentException>.from(_moment.exceptions);
    next[areaId] = current.copyWith(timerSeconds: seconds);
    unawaited(_saveExceptions(next));
  }

  void _removeException(String areaId) {
    final next = Map<String, _ExpertMomentException>.from(_moment.exceptions)
      ..remove(areaId);
    unawaited(_saveExceptions(next));
  }

  Future<void> _openAssignmentPicker() async {
    if (!_modernExpertCanEditInputBindings) {
      _showSnack('This build is missing the Expert moment assignment API.');
      return;
    }
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before assigning this moment.');
      return;
    }
    if (_switchesLoad.controls.isEmpty) {
      _showSnack('No controls loaded yet.');
      return;
    }

    final choice = await showModalBottomSheet<_MomentAssignmentChoice>(
      context: context,
      isScrollControlled: true,
      backgroundColor: CelestialColors.backgroundCard,
      builder: (_) => _MomentAssignmentSheet(
        moment: _moment,
        controls: _switchesLoad.controls,
        switchTypes: _switchesLoad.types,
        moments: _switchesLoad.moments,
        areaName: _areaName,
        sectionName: _sectionName,
      ),
    );
    if (choice == null || !mounted) return;

    final actionRef = 'set_${_moment.id}';
    final currentAction = choice.control.magicButtons[choice.event];
    if (currentAction == actionRef) {
      _showSnack('Already assigned to ${choice.control.name}.');
      return;
    }
    if (currentAction != null && currentAction.isNotEmpty) {
      final confirmed = await showDialog<bool>(
        context: context,
        builder: (context) => AlertDialog(
          backgroundColor: CelestialColors.backgroundCard,
          title: const Text(
            'Replace assignment',
            style: TextStyle(color: CelestialColors.textPrimary),
          ),
          content: Text(
            'Replace ${_magicAssignmentLabel(currentAction, _switchesLoad.moments)} on ${choice.control.name}?',
            style: const TextStyle(color: CelestialColors.textSecondary),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.of(context).pop(false),
              child: const Text('Cancel'),
            ),
            TextButton(
              onPressed: () => Navigator.of(context).pop(true),
              child: const Text('Replace'),
            ),
          ],
        ),
      );
      if (confirmed != true) return;
    }

    final nextMagic = Map<String, String?>.from(choice.control.magicButtons)
      ..[choice.event] = actionRef;
    await _saveAssignment(
      choice.control,
      nextMagic,
      busyKey: '${choice.control.id}:${choice.event}',
      success: 'Assigned ${_moment.name} to ${choice.control.name}.',
    );
  }

  Future<void> _removeAssignment(_ExpertMomentAssignment assignment) async {
    if (!_modernExpertCanEditInputBindings) {
      _showSnack('This build is missing the Expert moment assignment API.');
      return;
    }
    final nextMagic = Map<String, String?>.from(assignment.control.magicButtons)
      ..remove(assignment.event);
    await _saveAssignment(
      assignment.control,
      nextMagic,
      busyKey: assignment.key,
      success: 'Removed assignment from ${assignment.control.name}.',
    );
  }

  Future<void> _saveAssignment(
    _ExpertControl control,
    Map<String, String?> magicButtons, {
    required String busyKey,
    required String success,
  }) async {
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before changing assignments.');
      return;
    }

    HapticFeedback.selectionClick();
    setState(() {
      _assignmentBusyKey = busyKey;
      _error = null;
    });
    try {
      await client.updateSwitch(
        control.toSwitch().copyWith(magicButtons: magicButtons),
      );
      await _loadMoment();
      _showSnack(success);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: moment assignment save failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() => _error = 'Assignment save failed: $error');
      _showSnack('Assignment save failed: $error');
    } finally {
      if (mounted) setState(() => _assignmentBusyKey = null);
    }
  }

  List<String> _availableExceptionAreaIds() {
    return [
      for (final area in _areas)
        if (!_moment.exceptions.containsKey(area.id)) area.id,
    ];
  }

  String _areaName(String areaId) {
    for (final area in _areas) {
      if (area.id == areaId) return area.name;
    }
    return areaId;
  }

  String _sectionName(String sectionId) {
    return _sectionReachLabel(_areas, sectionId);
  }

  String _assignmentAreaLabel(_ExpertControl control) {
    final names = <String>[];
    for (final scope in control.scopes) {
      for (final areaId in scope.areaIds) {
        final name = _areaName(areaId);
        if (!names.contains(name)) names.add(name);
      }
      for (final sectionId in scope.sectionIds) {
        final name = _sectionName(sectionId);
        if (!names.contains(name)) names.add(name);
      }
    }
    if (names.isEmpty && control.areaName != null) names.add(control.areaName!);
    if (names.isEmpty) return 'No reach scope';
    if (names.length <= 3) return names.join(', ');
    return '${names.take(3).join(', ')}...';
  }

  void _showSnack(String message) {
    if (!mounted) return;
    ScaffoldMessenger.of(context)
        .showSnackBar(SnackBar(content: Text(message)));
  }

  @override
  Widget build(BuildContext context) {
    final timerMinutes = (_moment.timerSeconds / 60).round();
    final actionItems = _optionsWithCurrent(
      _moment.defaultAction,
      [for (final option in _momentActionOptions) option.$1],
    );
    final categoryItems = _optionsWithCurrent(
      _moment.category,
      [for (final option in _momentCategoryOptions) option.$1],
    );
    final assignments = _assignments;
    final availableExceptionIds = _availableExceptionAreaIds();
    final selectedNewArea = availableExceptionIds.contains(_newExceptionAreaId)
        ? _newExceptionAreaId
        : availableExceptionIds.isEmpty
            ? null
            : availableExceptionIds.first;

    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            _ExpertHeader(
              title: 'Moment',
              leading:
                  _BackButton(onPressed: () => Navigator.of(context).pop()),
              trailing: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  IconButton(
                    onPressed: _loading ? null : _loadMoment,
                    tooltip: 'Refresh moment',
                    icon: const Icon(Icons.refresh_rounded),
                    color: CelestialColors.textSecondary,
                    visualDensity: VisualDensity.compact,
                  ),
                  IconButton(
                    onPressed: _running ? null : _runMoment,
                    tooltip: 'Apply moment',
                    icon: const Icon(Icons.play_arrow_rounded),
                    color: _expertAccent,
                    visualDensity: VisualDensity.compact,
                  ),
                ],
              ),
            ),
            Expanded(
              child: ListView(
                padding: const EdgeInsets.fromLTRB(16, 0, 16, 28),
                children: [
                  _ExpertPanel(
                    padding: const EdgeInsets.all(12),
                    child: Row(
                      crossAxisAlignment: CrossAxisAlignment.center,
                      children: [
                        Container(
                          width: 44,
                          height: 44,
                          decoration: BoxDecoration(
                            color: _expertAccent.withValues(alpha: 0.12),
                            borderRadius: BorderRadius.circular(10),
                            border: Border.all(
                              color: _expertAccent.withValues(alpha: 0.4),
                            ),
                          ),
                          child: Center(
                            child: Text(
                              _momentIconText(_moment.icon),
                              style: const TextStyle(fontSize: 22),
                            ),
                          ),
                        ),
                        const SizedBox(width: 12),
                        Expanded(
                          child: TextField(
                            controller: _nameController,
                            enabled: !_loading,
                            style: const TextStyle(
                              color: CelestialColors.textPrimary,
                              fontSize: 20,
                              fontWeight: FontWeight.w800,
                            ),
                            decoration: const InputDecoration(
                              isDense: true,
                              border: InputBorder.none,
                              hintText: 'Moment name',
                              hintStyle: TextStyle(
                                color: CelestialColors.textSecondary,
                              ),
                            ),
                            onSubmitted: (_) => unawaited(_saveName()),
                            onEditingComplete: () => unawaited(_saveName()),
                          ),
                        ),
                        IconButton(
                          onPressed:
                              _saving ? null : () => unawaited(_saveName()),
                          tooltip: 'Save name',
                          icon: const Icon(Icons.check_rounded),
                          color: _saving
                              ? CelestialColors.textSecondary
                              : _changedAccent,
                        ),
                      ],
                    ),
                  ),
                  if (_loading || _saving || _running) ...[
                    const SizedBox(height: 8),
                    _ExpertPanel(
                      padding: const EdgeInsets.symmetric(
                        horizontal: 12,
                        vertical: 9,
                      ),
                      child: Row(
                        children: [
                          const SizedBox(
                            width: 16,
                            height: 16,
                            child: CircularProgressIndicator(strokeWidth: 2),
                          ),
                          const SizedBox(width: 8),
                          Text(
                            _running
                                ? 'Applying moment...'
                                : _saving
                                    ? 'Saving...'
                                    : 'Loading...',
                            style: const TextStyle(
                              color: CelestialColors.textSecondary,
                              fontSize: 12,
                              fontWeight: FontWeight.w700,
                            ),
                          ),
                        ],
                      ),
                    ),
                  ],
                  if (_error != null) ...[
                    const SizedBox(height: 8),
                    _ExpertPanel(
                      child: Text(
                        _error!,
                        style: const TextStyle(
                          color: _dangerAccent,
                          fontSize: 12,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                    ),
                  ],
                  const SizedBox(height: 12),
                  _ExpertPanel(
                    child: Column(
                      children: [
                        _MomentFieldRow(
                          label: 'Category',
                          child: _CompactDropdown<String>(
                            value: _moment.category,
                            items: categoryItems,
                            itemLabel: _momentCategoryLabel,
                            onChanged: (value) => unawaited(
                              _setCategory(value),
                            ),
                          ),
                        ),
                        _MomentFieldRow(
                          label: 'Action',
                          child: _CompactDropdown<String>(
                            value: _moment.defaultAction,
                            items: actionItems,
                            itemLabel: _momentActionLabel,
                            onChanged: (value) => unawaited(
                              _setDefaultAction(value),
                            ),
                          ),
                        ),
                        _MomentFieldRow(
                          label: 'Auto-off',
                          child: _MomentTimerStepper(
                            minutes: timerMinutes,
                            onChanged: (value) =>
                                unawaited(_setTimerMinutes(value)),
                          ),
                        ),
                        const SizedBox(height: 4),
                        Align(
                          alignment: Alignment.centerLeft,
                          child: Text(
                            'ID: ${_moment.id}',
                            style: const TextStyle(
                              color: CelestialColors.textSecondary,
                              fontSize: 11,
                            ),
                          ),
                        ),
                      ],
                    ),
                  ),
                  const SizedBox(height: 12),
                  _ExpertPanel(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        const Text(
                          'Icon',
                          style: TextStyle(
                            color: CelestialColors.textPrimary,
                            fontSize: 15,
                            fontWeight: FontWeight.w800,
                          ),
                        ),
                        const SizedBox(height: 10),
                        Wrap(
                          spacing: 8,
                          runSpacing: 8,
                          children: [
                            for (final option in _momentIconOptions)
                              _MomentIconChoice(
                                icon: option.$1,
                                label: option.$2,
                                selected: _moment.icon == option.$1,
                                onSelected: () =>
                                    unawaited(_setIcon(option.$1)),
                              ),
                          ],
                        ),
                      ],
                    ),
                  ),
                  const SizedBox(height: 12),
                  _ExpertPanel(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        Row(
                          children: [
                            const Text(
                              'Exceptions',
                              style: TextStyle(
                                color: CelestialColors.textPrimary,
                                fontSize: 15,
                                fontWeight: FontWeight.w800,
                              ),
                            ),
                            const Spacer(),
                            Text(
                              _moment.exceptions.isEmpty
                                  ? 'none'
                                  : '${_moment.exceptions.length}',
                              style: const TextStyle(
                                color: CelestialColors.textSecondary,
                                fontSize: 12,
                                fontWeight: FontWeight.w700,
                              ),
                            ),
                          ],
                        ),
                        const SizedBox(height: 10),
                        if (_moment.exceptions.isEmpty)
                          const Text(
                            'No area-specific overrides.',
                            style: TextStyle(
                              color: CelestialColors.textSecondary,
                              fontSize: 12,
                            ),
                          )
                        else
                          for (final entry in _moment.exceptions.entries)
                            _MomentExceptionRow(
                              areaName: _areaName(entry.key),
                              exception: entry.value,
                              onActionChanged: (value) =>
                                  _updateExceptionAction(entry.key, value),
                              onTimerChanged: (value) =>
                                  _updateExceptionMinutes(entry.key, value),
                              onRemove: () => _removeException(entry.key),
                            ),
                        const SizedBox(height: 12),
                        if (selectedNewArea == null)
                          const Text(
                            'Every visible area already has an exception.',
                            style: TextStyle(
                              color: CelestialColors.textSecondary,
                              fontSize: 12,
                            ),
                          )
                        else
                          Wrap(
                            spacing: 8,
                            runSpacing: 8,
                            crossAxisAlignment: WrapCrossAlignment.center,
                            children: [
                              _CompactDropdown<String>(
                                value: selectedNewArea,
                                items: availableExceptionIds,
                                itemLabel: _areaName,
                                onChanged: (value) =>
                                    setState(() => _newExceptionAreaId = value),
                              ),
                              _CompactDropdown<String>(
                                value: _newExceptionAction,
                                items: [
                                  for (final option in _momentActionOptions)
                                    option.$1,
                                ],
                                itemLabel: _momentActionLabel,
                                onChanged: (value) => setState(
                                  () => _newExceptionAction = value,
                                ),
                              ),
                              _MomentTimerStepper(
                                minutes: _newExceptionMinutes,
                                onChanged: (value) => setState(
                                  () => _newExceptionMinutes = value,
                                ),
                              ),
                              OutlinedButton.icon(
                                onPressed: _saving ? null : _addException,
                                icon: const Icon(Icons.add_rounded, size: 18),
                                label: const Text('Add'),
                                style: OutlinedButton.styleFrom(
                                  foregroundColor: _expertAccent,
                                  side: const BorderSide(
                                    color: CelestialColors.orbitRing,
                                  ),
                                  shape: RoundedRectangleBorder(
                                    borderRadius: BorderRadius.circular(8),
                                  ),
                                ),
                              ),
                            ],
                          ),
                      ],
                    ),
                  ),
                  const SizedBox(height: 12),
                  _ExpertPanel(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        Row(
                          children: [
                            const Expanded(
                              child: Text(
                                'Usage',
                                style: TextStyle(
                                  color: CelestialColors.textPrimary,
                                  fontSize: 15,
                                  fontWeight: FontWeight.w800,
                                ),
                              ),
                            ),
                            Text(
                              assignments.isEmpty
                                  ? 'none'
                                  : '${assignments.length}',
                              style: const TextStyle(
                                color: CelestialColors.textSecondary,
                                fontSize: 12,
                                fontWeight: FontWeight.w700,
                              ),
                            ),
                          ],
                        ),
                        const SizedBox(height: 10),
                        if (assignments.isEmpty)
                          const Text(
                            'Not assigned to any known switch.',
                            style: TextStyle(
                              color: CelestialColors.textSecondary,
                              fontSize: 12,
                            ),
                          )
                        else
                          for (final assignment in assignments)
                            _MomentUsageRow(
                              assignment: assignment,
                              areaLabel:
                                  _assignmentAreaLabel(assignment.control),
                              busy: _assignmentBusyKey == assignment.key,
                              onRemove: _modernExpertCanEditInputBindings
                                  ? () => unawaited(
                                        _removeAssignment(assignment),
                                      )
                                  : null,
                            ),
                        const SizedBox(height: 10),
                        Align(
                          alignment: Alignment.centerLeft,
                          child: OutlinedButton.icon(
                            onPressed: _assignmentBusyKey == null &&
                                    _modernExpertCanEditInputBindings
                                ? () => unawaited(_openAssignmentPicker())
                                : null,
                            icon: const Icon(Icons.add_rounded, size: 18),
                            label: const Text('Assign to control'),
                            style: OutlinedButton.styleFrom(
                              foregroundColor: _expertAccent,
                              side: const BorderSide(
                                color: CelestialColors.orbitRing,
                              ),
                              shape: RoundedRectangleBorder(
                                borderRadius: BorderRadius.circular(8),
                              ),
                            ),
                          ),
                        ),
                      ],
                    ),
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _ExpertSwitchesScreen extends StatefulWidget {
  final _CircadianExpertClient? client;
  final List<_ExpertZone> zones;
  final bool showBackButton;

  const _ExpertSwitchesScreen({
    required this.client,
    required this.zones,
    this.showBackButton = true,
  });

  @override
  State<_ExpertSwitchesScreen> createState() => _ExpertSwitchesScreenState();
}

class _ExpertSwitchesScreenState extends State<_ExpertSwitchesScreen> {
  _ExpertSwitchesLoad _load = const _ExpertSwitchesLoad.empty();
  bool _loading = true;
  String? _error;
  String _viewMode = 'all';
  String _sortMode = 'recent';
  String _categoryFilter = 'all';
  String _query = '';
  String? _busyControlId;
  Timer? _controlsRefreshTimer;
  bool _refreshingControls = false;

  @override
  void initState() {
    super.initState();
    unawaited(_loadSwitches());
  }

  @override
  void dispose() {
    _controlsRefreshTimer?.cancel();
    super.dispose();
  }

  Future<void> _loadSwitches() async {
    final client = widget.client;
    if (client == null) {
      _controlsRefreshTimer?.cancel();
      setState(() {
        _loading = false;
        _error = 'Connect to RhythmOS to manage controls.';
      });
      return;
    }

    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final load = await client.fetchSwitches();
      if (!mounted) return;
      setState(() {
        _load = load;
        _loading = false;
      });
      _restartControlsRefreshTimer(load.refreshIntervalSeconds);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: controls load failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() {
        _loading = false;
        _error = 'Controls load failed: $error';
      });
    }
  }

  void _restartControlsRefreshTimer(int seconds) {
    _controlsRefreshTimer?.cancel();
    final client = widget.client;
    if (client == null) return;
    final interval = Duration(seconds: seconds.clamp(1, 60).toInt());
    _controlsRefreshTimer = Timer.periodic(interval, (_) {
      unawaited(_refreshControls());
    });
  }

  Future<void> _refreshControls() async {
    final client = widget.client;
    if (client == null ||
        _loading ||
        _refreshingControls ||
        _load.controls.isEmpty) {
      return;
    }

    _refreshingControls = true;
    try {
      final refresh = await client.fetchControlsRefresh();
      if (!mounted) return;
      final next = _load.withRefresh(refresh);
      if (!identical(next, _load)) {
        setState(() => _load = next);
      }
    } catch (error) {
      debugPrint('CircadianExpert: controls refresh failed: $error');
    } finally {
      _refreshingControls = false;
    }
  }

  Future<void> _openSwitch(_ExpertControl control) async {
    if (!control.isSwitch) {
      await Navigator.of(context).push(
        MaterialPageRoute(
          builder: (_) => _ExpertControlDetailScreen(
            client: widget.client,
            initialControl: control,
            zones: widget.zones,
            defaultPauseMinutes: _load.defaultPauseMinutes,
          ),
        ),
      );
      if (mounted) unawaited(_loadSwitches());
      return;
    }
    await Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) => _ExpertSwitchDetailScreen(
          client: widget.client,
          initialSwitch: control.toSwitch(),
          switchTypes: _load.types,
          moments: _load.moments,
          zones: widget.zones,
        ),
      ),
    );
    if (mounted) unawaited(_loadSwitches());
  }

  Future<void> _openSwitchmap() async {
    await Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) => _ExpertSwitchmapScreen(client: widget.client),
      ),
    );
    if (mounted) unawaited(_loadSwitches());
  }

  Future<void> _openAddControlPicker() async {
    if (!_modernExpertCanAddControlSources) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(
          content: Text('This build is missing the Expert controls API.'),
        ),
      );
      return;
    }
    final client = widget.client;
    if (client == null) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(
          content: Text('Connect to RhythmOS before adding controls.'),
        ),
      );
      return;
    }

    final addedControlId = await showModalBottomSheet<String>(
      context: context,
      isScrollControlled: true,
      useSafeArea: true,
      backgroundColor: Colors.transparent,
      builder: (_) => _AddControlSourceSheet(client: client),
    );
    if (addedControlId == null || !mounted) return;

    await _loadSwitches();
    if (!mounted) return;
    _ExpertControl? addedControl;
    for (final control in _load.controls) {
      if (control.id == addedControlId) {
        addedControl = control;
        break;
      }
    }
    if (addedControl != null) {
      await _openSwitch(addedControl);
    } else if (mounted) {
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('Added $addedControlId to controls.')),
      );
    }
  }

  Future<void> _deleteSwitch(_ExpertControl control) async {
    final client = widget.client;
    if (client == null || !control.isSwitch) return;
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: const Text(
          'Delete switch',
          style: TextStyle(color: CelestialColors.textPrimary),
        ),
        content: Text(
          'Delete "${control.name}" from expert switch config?',
          style: const TextStyle(color: CelestialColors.textSecondary),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('Cancel'),
          ),
          TextButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text(
              'Delete',
              style: TextStyle(color: _dangerAccent),
            ),
          ),
        ],
      ),
    );
    if (confirmed != true) return;

    HapticFeedback.selectionClick();
    setState(() => _busyControlId = control.id);
    try {
      await client.deleteSwitch(control.id);
      await _loadSwitches();
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: switch delete failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('Switch delete failed: $error')),
      );
    } finally {
      if (mounted) setState(() => _busyControlId = null);
    }
  }

  Future<void> _togglePause(_ExpertControl control) async {
    final client = widget.client;
    if (client == null) return;

    final nextInactive = !control.inactive;
    final nextUntil =
        nextInactive ? _pauseUntilIso(_load.defaultPauseMinutes) : null;

    HapticFeedback.selectionClick();
    setState(() => _busyControlId = control.id);
    try {
      await client.setControlPause(
        control.pauseEndpointKey,
        inactive: nextInactive,
        inactiveUntil: nextUntil,
      );
      if (!mounted) return;
      setState(() {
        _load = _load.withUpdatedControl(
          control.copyWith(
            inactive: nextInactive,
            inactiveUntil: nextUntil,
            status: _controlStatusWithPause(control.status, nextInactive),
          ),
        );
      });
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: control pause failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('Pause update failed: $error')),
      );
    } finally {
      if (mounted) setState(() => _busyControlId = null);
    }
  }

  List<_ExpertControl> _visibleControls() {
    final query = _query.trim().toLowerCase();
    final controls = [
      for (final control in _load.controls)
        if (_matchesControlView(control) &&
            (_categoryFilter == 'all' || control.category == _categoryFilter) &&
            (query.isEmpty ||
                control.name.toLowerCase().contains(query) ||
                control.id.toLowerCase().contains(query) ||
                control.typeName.toLowerCase().contains(query) ||
                control.categoryLabel.toLowerCase().contains(query) ||
                (control.areaName ?? '').toLowerCase().contains(query) ||
                (control.manufacturer ?? '').toLowerCase().contains(query) ||
                (control.model ?? '').toLowerCase().contains(query)))
          control,
    ];
    controls.sort((a, b) {
      final compare = switch (_sortMode) {
        'area' => (a.areaName ?? '').compareTo(b.areaName ?? ''),
        'name' => a.name.toLowerCase().compareTo(b.name.toLowerCase()),
        'type' => a.categoryLabel.compareTo(b.categoryLabel),
        'battery' => (a.batteryLevel ?? 101).compareTo(b.batteryLevel ?? 101),
        'status' => _controlStatusRank(a.status).compareTo(
            _controlStatusRank(b.status),
          ),
        _ => a.name.toLowerCase().compareTo(b.name.toLowerCase()),
      };
      return compare != 0
          ? compare
          : a.name.toLowerCase().compareTo(b.name.toLowerCase());
    });
    if (_sortMode == 'recent') {
      controls.sort((a, b) => (b.lastActionTime?.millisecondsSinceEpoch ?? 0)
          .compareTo(a.lastActionTime?.millisecondsSinceEpoch ?? 0));
    }
    return controls;
  }

  bool _matchesControlView(_ExpertControl control) {
    return switch (_viewMode) {
      'setup' => control.status == 'not_configured',
      'batteries' => control.batteryLevel != null,
      'paused' => control.inactive,
      _ => true,
    };
  }

  List<String> _categoryOptions() {
    final values = {
      for (final control in _load.controls) control.category,
    }.where((value) => value.isNotEmpty).toList()
      ..sort((a, b) => _controlCategoryLabel(a).compareTo(
            _controlCategoryLabel(b),
          ));
    return ['all', ...values];
  }

  @override
  Widget build(BuildContext context) {
    final controls = _visibleControls();
    final categoryOptions = _categoryOptions();
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            _ExpertHeader(
              title: 'Controls',
              leading: widget.showBackButton
                  ? _BackButton(onPressed: () => Navigator.of(context).pop())
                  : const _ExpertModeBadge(),
              leadingWidth: widget.showBackButton ? 96 : 48,
              trailingWidth: 120,
              trailing: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  IconButton(
                    onPressed: _modernExpertCanAddControlSources
                        ? _openAddControlPicker
                        : null,
                    tooltip: 'Add control source',
                    icon: const Icon(Icons.add_rounded),
                    color: _expertAccent,
                    visualDensity: VisualDensity.compact,
                  ),
                  IconButton(
                    onPressed: _openSwitchmap,
                    tooltip: 'Switch map',
                    icon: const Icon(Icons.account_tree_rounded),
                    color: _expertAccent,
                    visualDensity: VisualDensity.compact,
                  ),
                  IconButton(
                    onPressed: _loading ? null : _loadSwitches,
                    tooltip: 'Refresh controls',
                    icon: const Icon(Icons.refresh_rounded),
                    color: CelestialColors.textSecondary,
                    visualDensity: VisualDensity.compact,
                  ),
                ],
              ),
            ),
            Expanded(
              child: ListView(
                padding: const EdgeInsets.fromLTRB(16, 0, 16, 28),
                children: [
                  _ExpertPanel(
                    padding: const EdgeInsets.all(12),
                    child: Column(
                      children: [
                        TextField(
                          style: const TextStyle(
                            color: CelestialColors.textPrimary,
                            fontSize: 13,
                            fontWeight: FontWeight.w600,
                          ),
                          decoration: InputDecoration(
                            isDense: true,
                            hintText: 'Filter controls',
                            hintStyle: const TextStyle(
                              color: CelestialColors.textSecondary,
                            ),
                            prefixIcon: const Icon(
                              Icons.search_rounded,
                              color: CelestialColors.textSecondary,
                              size: 18,
                            ),
                            filled: true,
                            fillColor: _panelColor,
                            enabledBorder: OutlineInputBorder(
                              borderRadius: BorderRadius.circular(8),
                              borderSide: const BorderSide(
                                color: CelestialColors.orbitRing,
                              ),
                            ),
                            focusedBorder: OutlineInputBorder(
                              borderRadius: BorderRadius.circular(8),
                              borderSide:
                                  const BorderSide(color: _expertAccent),
                            ),
                          ),
                          onChanged: (value) => setState(() => _query = value),
                        ),
                        const SizedBox(height: 10),
                        _ControlViewChips(
                          value: _viewMode,
                          onChanged: (value) => setState(() {
                            _viewMode = value;
                            if (value == 'batteries') {
                              _sortMode = 'battery';
                            } else if (value == 'paused') {
                              _sortMode = 'status';
                            } else if (_sortMode == 'battery' ||
                                _sortMode == 'status') {
                              _sortMode = 'recent';
                            }
                          }),
                        ),
                        const SizedBox(height: 10),
                        Row(
                          children: [
                            Expanded(
                              child: _ControlCategoryDropdown(
                                value: _categoryFilter,
                                items: categoryOptions,
                                onChanged: (value) =>
                                    setState(() => _categoryFilter = value),
                              ),
                            ),
                            const SizedBox(width: 8),
                            Text(
                              '${controls.length} ${controls.length == 1 ? 'control' : 'controls'}',
                              style: const TextStyle(
                                color: CelestialColors.textSecondary,
                                fontSize: 12,
                                fontWeight: FontWeight.w700,
                              ),
                            ),
                          ],
                        ),
                        const SizedBox(height: 10),
                        Row(
                          children: [
                            Expanded(
                              child: _ControlSortChips(
                                value: _sortMode,
                                viewMode: _viewMode,
                                onChanged: (value) =>
                                    setState(() => _sortMode = value),
                              ),
                            ),
                            if (_loading) ...[
                              const SizedBox(width: 10),
                              const SizedBox(
                                width: 18,
                                height: 18,
                                child:
                                    CircularProgressIndicator(strokeWidth: 2),
                              ),
                            ],
                          ],
                        ),
                        if (_error != null) ...[
                          const SizedBox(height: 10),
                          Align(
                            alignment: Alignment.centerLeft,
                            child: Text(
                              _error!,
                              style: const TextStyle(
                                color: _dangerAccent,
                                fontSize: 12,
                                fontWeight: FontWeight.w600,
                              ),
                            ),
                          ),
                        ],
                      ],
                    ),
                  ),
                  const SizedBox(height: 12),
                  if (!_loading && controls.isEmpty)
                    const _ExpertPanel(
                      child: Text(
                        'No controls match this view.',
                        style: TextStyle(
                          color: CelestialColors.textSecondary,
                          fontSize: 13,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                    )
                  else
                    for (final control in controls)
                      _SwitchRow(
                        control: control,
                        busy: _busyControlId == control.id,
                        pulseWindowHours: _load.controlsPulseWindowHours,
                        recentWindowMinutes: _load.controlsRecentWindowMinutes,
                        onOpen: () => _openSwitch(control),
                        onTogglePause: () => _togglePause(control),
                        onDelete: control.isSwitch
                            ? () => _deleteSwitch(control)
                            : null,
                      ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _AddControlSourceSheet extends StatefulWidget {
  final _CircadianExpertClient client;

  const _AddControlSourceSheet({required this.client});

  @override
  State<_AddControlSourceSheet> createState() => _AddControlSourceSheetState();
}

class _AddControlSourceSheetState extends State<_AddControlSourceSheet> {
  final TextEditingController _searchController = TextEditingController();
  Timer? _searchTimer;
  List<_ExpertControlSourceDevice> _devices = const [];
  _ExpertControlSourceDevice? _selectedDevice;
  final Set<String> _selectedSensors = {};
  bool _loading = true;
  bool _adding = false;
  String? _error;
  String? _reportingDeviceId;

  @override
  void initState() {
    super.initState();
    unawaited(_searchDevices());
  }

  @override
  void dispose() {
    _searchTimer?.cancel();
    _searchController.dispose();
    super.dispose();
  }

  void _queueSearch(String query) {
    _searchTimer?.cancel();
    _searchTimer = Timer(const Duration(milliseconds: 220), () {
      unawaited(_searchDevices(query: query));
    });
  }

  Future<void> _searchDevices({String? query}) async {
    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final devices = await widget.client.searchControlSourceDevices(
        query ?? _searchController.text,
      );
      if (!mounted) return;
      setState(() {
        _devices = devices;
        if (_selectedDevice != null &&
            !devices.any(
              (device) => device.deviceId == _selectedDevice!.deviceId,
            )) {
          _selectedDevice = null;
          _selectedSensors.clear();
        }
        _loading = false;
      });
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: device search failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() {
        _loading = false;
        _error = 'Device search failed: $error';
      });
    }
  }

  void _selectDevice(_ExpertControlSourceDevice device) {
    HapticFeedback.selectionClick();
    setState(() {
      _selectedDevice = device;
      _selectedSensors.clear();
    });
  }

  void _toggleSensor(String entityId) {
    HapticFeedback.selectionClick();
    setState(() {
      if (_selectedSensors.contains(entityId)) {
        _selectedSensors.remove(entityId);
      } else {
        _selectedSensors.add(entityId);
      }
    });
  }

  Future<void> _reportDevice(_ExpertControlSourceDevice device) async {
    setState(() {
      _reportingDeviceId = device.deviceId;
      _error = null;
    });
    try {
      await widget.client.reportControlDevice(device.deviceId);
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('Reported ${device.name}.')),
      );
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: device report failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() => _error = 'Device report failed: $error');
    } finally {
      if (mounted) setState(() => _reportingDeviceId = null);
    }
  }

  Future<void> _addSelectedControl() async {
    final device = _selectedDevice;
    if (device == null || _selectedSensors.isEmpty) return;

    HapticFeedback.selectionClick();
    setState(() {
      _adding = true;
      _error = null;
    });
    try {
      final id = await widget.client.addControlSource(
        device: device,
        triggerEntities: _selectedSensors.toList()..sort(),
      );
      if (!mounted) return;
      Navigator.of(context).pop(id);
    } on DioException catch (error, stackTrace) {
      final body = _asMapOrNull(error.response?.data);
      final errorText = _stringValue(body?['error']);
      final configuredId = _stringValue(body?['id']);
      if (error.response?.statusCode == 409 &&
          errorText == 'Device already configured' &&
          configuredId != null) {
        if (!mounted) return;
        Navigator.of(context).pop(configuredId);
        return;
      }
      debugPrint('CircadianExpert: add control failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() => _error = 'Add control failed: ${error.message}');
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: add control failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() => _error = 'Add control failed: $error');
    } finally {
      if (mounted) setState(() => _adding = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final selectedDevice = _selectedDevice;
    return DraggableScrollableSheet(
      initialChildSize: 0.86,
      minChildSize: 0.58,
      maxChildSize: 0.96,
      builder: (context, scrollController) {
        return Container(
          decoration: const BoxDecoration(
            color: CelestialColors.backgroundCard,
            borderRadius: BorderRadius.vertical(top: Radius.circular(18)),
            border: Border(
              top: BorderSide(color: CelestialColors.orbitRing),
            ),
          ),
          child: Column(
            children: [
              Padding(
                padding: const EdgeInsets.fromLTRB(16, 10, 16, 8),
                child: Column(
                  children: [
                    Container(
                      width: 42,
                      height: 4,
                      decoration: BoxDecoration(
                        color: CelestialColors.orbitRing,
                        borderRadius: BorderRadius.circular(999),
                      ),
                    ),
                    const SizedBox(height: 14),
                    Row(
                      children: [
                        if (selectedDevice != null)
                          IconButton(
                            onPressed: _adding
                                ? null
                                : () => setState(() {
                                      _selectedDevice = null;
                                      _selectedSensors.clear();
                                    }),
                            tooltip: 'Back to devices',
                            icon: const Icon(Icons.arrow_back_rounded),
                            color: CelestialColors.textSecondary,
                            visualDensity: VisualDensity.compact,
                          )
                        else
                          const Icon(
                            Icons.sensors_rounded,
                            color: _expertAccent,
                            size: 22,
                          ),
                        const SizedBox(width: 8),
                        const Expanded(
                          child: Text(
                            'Add control source',
                            style: TextStyle(
                              color: CelestialColors.textPrimary,
                              fontSize: 17,
                              fontWeight: FontWeight.w800,
                            ),
                          ),
                        ),
                        IconButton(
                          onPressed: _adding
                              ? null
                              : () => Navigator.of(context).pop(),
                          tooltip: 'Close',
                          icon: const Icon(Icons.close_rounded),
                          color: CelestialColors.textSecondary,
                          visualDensity: VisualDensity.compact,
                        ),
                      ],
                    ),
                    if (selectedDevice == null) ...[
                      const SizedBox(height: 10),
                      TextField(
                        controller: _searchController,
                        onChanged: _queueSearch,
                        style: const TextStyle(
                          color: CelestialColors.textPrimary,
                          fontSize: 13,
                          fontWeight: FontWeight.w600,
                        ),
                        decoration: InputDecoration(
                          isDense: true,
                          hintText: 'Search devices',
                          hintStyle: const TextStyle(
                            color: CelestialColors.textSecondary,
                          ),
                          prefixIcon: const Icon(
                            Icons.search_rounded,
                            color: CelestialColors.textSecondary,
                            size: 18,
                          ),
                          suffixIcon: _loading
                              ? const Padding(
                                  padding: EdgeInsets.all(13),
                                  child: SizedBox(
                                    width: 14,
                                    height: 14,
                                    child: CircularProgressIndicator(
                                      strokeWidth: 2,
                                    ),
                                  ),
                                )
                              : null,
                          filled: true,
                          fillColor: _panelColor,
                          enabledBorder: OutlineInputBorder(
                            borderRadius: BorderRadius.circular(8),
                            borderSide: const BorderSide(
                              color: CelestialColors.orbitRing,
                            ),
                          ),
                          focusedBorder: OutlineInputBorder(
                            borderRadius: BorderRadius.circular(8),
                            borderSide: const BorderSide(
                              color: _expertAccent,
                            ),
                          ),
                        ),
                      ),
                    ],
                    if (_error != null) ...[
                      const SizedBox(height: 10),
                      Align(
                        alignment: Alignment.centerLeft,
                        child: Text(
                          _error!,
                          style: const TextStyle(
                            color: _dangerAccent,
                            fontSize: 12,
                            fontWeight: FontWeight.w600,
                          ),
                        ),
                      ),
                    ],
                  ],
                ),
              ),
              Expanded(
                child: ListView(
                  controller: scrollController,
                  padding: const EdgeInsets.fromLTRB(16, 4, 16, 16),
                  children: [
                    if (selectedDevice == null)
                      _buildDeviceList()
                    else
                      _buildSensorPicker(selectedDevice),
                  ],
                ),
              ),
              if (selectedDevice != null)
                Container(
                  padding: const EdgeInsets.fromLTRB(16, 10, 16, 16),
                  decoration: const BoxDecoration(
                    color: CelestialColors.backgroundCard,
                    border: Border(
                      top: BorderSide(color: CelestialColors.orbitRing),
                    ),
                  ),
                  child: Row(
                    children: [
                      Expanded(
                        child: OutlinedButton(
                          onPressed: _adding
                              ? null
                              : () => setState(() {
                                    _selectedDevice = null;
                                    _selectedSensors.clear();
                                  }),
                          style: OutlinedButton.styleFrom(
                            foregroundColor: CelestialColors.textSecondary,
                            side: const BorderSide(
                              color: CelestialColors.orbitRing,
                            ),
                          ),
                          child: const Text('Back'),
                        ),
                      ),
                      const SizedBox(width: 10),
                      Expanded(
                        child: FilledButton.icon(
                          onPressed: !_adding && _selectedSensors.isNotEmpty
                              ? _addSelectedControl
                              : null,
                          icon: _adding
                              ? const SizedBox(
                                  width: 16,
                                  height: 16,
                                  child: CircularProgressIndicator(
                                    strokeWidth: 2,
                                  ),
                                )
                              : const Icon(Icons.add_rounded),
                          label: Text(_adding ? 'Adding...' : 'Add'),
                          style: FilledButton.styleFrom(
                            backgroundColor: _expertAccent,
                            foregroundColor: const Color(0xFF1A120C),
                          ),
                        ),
                      ),
                    ],
                  ),
                ),
            ],
          ),
        );
      },
    );
  }

  Widget _buildDeviceList() {
    if (_loading && _devices.isEmpty) {
      return const _ExpertPanel(
        child: Center(
          child: Padding(
            padding: EdgeInsets.symmetric(vertical: 18),
            child: CircularProgressIndicator(strokeWidth: 2),
          ),
        ),
      );
    }
    if (_devices.isEmpty) {
      return const _ExpertPanel(
        child: Text(
          'No control-source devices found.',
          style: TextStyle(
            color: CelestialColors.textSecondary,
            fontSize: 13,
            fontWeight: FontWeight.w600,
          ),
        ),
      );
    }

    return Column(
      children: [
        for (final device in _devices)
          _ControlSourceDeviceRow(
            device: device,
            reporting: _reportingDeviceId == device.deviceId,
            onSelect: () => _selectDevice(device),
            onReport: () => unawaited(_reportDevice(device)),
          ),
      ],
    );
  }

  Widget _buildSensorPicker(_ExpertControlSourceDevice device) {
    final sensors = List<_ExpertControlSourceSensor>.from(device.binarySensors)
      ..sort((a, b) {
        final rank = _controlSourceSensorRank(a).compareTo(
          _controlSourceSensorRank(b),
        );
        return rank != 0
            ? rank
            : a.name.toLowerCase().compareTo(b.name.toLowerCase());
      });

    return _ExpertPanel(
      padding: const EdgeInsets.all(12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            device.name,
            style: const TextStyle(
              color: CelestialColors.textPrimary,
              fontSize: 15,
              fontWeight: FontWeight.w800,
            ),
          ),
          const SizedBox(height: 3),
          Text(
            _controlSourceDeviceMeta(device),
            style: const TextStyle(
              color: CelestialColors.textSecondary,
              fontSize: 11,
              fontWeight: FontWeight.w600,
            ),
          ),
          const SizedBox(height: 12),
          const Text(
            'Select trigger entities',
            style: TextStyle(
              color: CelestialColors.textSecondary,
              fontSize: 12,
              fontWeight: FontWeight.w800,
            ),
          ),
          const SizedBox(height: 8),
          if (sensors.isEmpty)
            const Text(
              'No binary sensors were reported for this device.',
              style: TextStyle(
                color: CelestialColors.textSecondary,
                fontSize: 12,
              ),
            )
          else
            Wrap(
              spacing: 8,
              runSpacing: 8,
              children: [
                for (final sensor in sensors)
                  FilterChip(
                    selected: _selectedSensors.contains(sensor.entityId),
                    onSelected:
                        _adding ? null : (_) => _toggleSensor(sensor.entityId),
                    label: Text(
                      sensor.label,
                      overflow: TextOverflow.ellipsis,
                    ),
                    tooltip: sensor.entityId,
                    backgroundColor: _panel2Color.withValues(alpha: 0.6),
                    selectedColor: _expertAccent.withValues(alpha: 0.22),
                    checkmarkColor: _expertAccent,
                    side: BorderSide(
                      color: _selectedSensors.contains(sensor.entityId)
                          ? _expertAccent.withValues(alpha: 0.7)
                          : CelestialColors.orbitRing,
                    ),
                    labelStyle: TextStyle(
                      color: _selectedSensors.contains(sensor.entityId)
                          ? CelestialColors.textPrimary
                          : CelestialColors.textSecondary,
                      fontSize: 12,
                      fontWeight: FontWeight.w700,
                    ),
                  ),
              ],
            ),
        ],
      ),
    );
  }
}

class _ControlSourceDeviceRow extends StatelessWidget {
  final _ExpertControlSourceDevice device;
  final bool reporting;
  final VoidCallback onSelect;
  final VoidCallback onReport;

  const _ControlSourceDeviceRow({
    required this.device,
    required this.reporting,
    required this.onSelect,
    required this.onReport,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 8),
      child: _ExpertPanel(
        padding: const EdgeInsets.fromLTRB(12, 10, 8, 10),
        child: InkWell(
          onTap: onSelect,
          borderRadius: BorderRadius.circular(8),
          child: Row(
            children: [
              Container(
                width: 32,
                height: 32,
                alignment: Alignment.center,
                decoration: BoxDecoration(
                  color: _expertAccent.withValues(alpha: 0.14),
                  borderRadius: BorderRadius.circular(8),
                  border: Border.all(
                    color: _expertAccent.withValues(alpha: 0.45),
                  ),
                ),
                child: const Icon(
                  Icons.sensors_rounded,
                  color: _expertAccent,
                  size: 18,
                ),
              ),
              const SizedBox(width: 10),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      device.name,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(
                        color: CelestialColors.textPrimary,
                        fontSize: 13,
                        fontWeight: FontWeight.w800,
                      ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      _controlSourceDeviceMeta(device),
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(
                        color: CelestialColors.textSecondary,
                        fontSize: 11,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                  ],
                ),
              ),
              const SizedBox(width: 8),
              TextButton(
                onPressed: reporting ? null : onReport,
                style: TextButton.styleFrom(
                  foregroundColor: _changedAccent,
                  visualDensity: VisualDensity.compact,
                ),
                child: reporting
                    ? const SizedBox(
                        width: 14,
                        height: 14,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      )
                    : const Text('Report'),
              ),
              const Icon(
                Icons.chevron_right_rounded,
                color: CelestialColors.textSecondary,
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _ExpertSwitchmapScreen extends StatefulWidget {
  final _CircadianExpertClient? client;

  const _ExpertSwitchmapScreen({required this.client});

  @override
  State<_ExpertSwitchmapScreen> createState() => _ExpertSwitchmapScreenState();
}

class _ExpertSwitchmapScreenState extends State<_ExpertSwitchmapScreen> {
  _ExpertSwitchmapLoad _load = const _ExpertSwitchmapLoad.empty();
  bool _loading = true;
  bool _saving = false;
  String? _error;
  String? _typeId;
  Map<String, Object?> _draftMappings = const {};

  @override
  void initState() {
    super.initState();
    unawaited(_loadSwitchmap());
  }

  Future<void> _loadSwitchmap({String? keepTypeId}) async {
    final client = widget.client;
    if (client == null) {
      setState(() {
        _loading = false;
        _error = 'Connect to RhythmOS to edit switch mappings.';
      });
      return;
    }

    setState(() {
      _loading = true;
      _error = null;
    });
    try {
      final load = await client.fetchSwitchmap();
      if (!mounted) return;
      final nextTypeId =
          keepTypeId != null && load.types.containsKey(keepTypeId)
              ? keepTypeId
              : load.types.keys.isEmpty
                  ? null
                  : load.types.keys.first;
      setState(() {
        _load = load;
        _typeId = nextTypeId;
        _draftMappings = Map<String, Object?>.from(
          nextTypeId == null
              ? const {}
              : load.types[nextTypeId]?.effectiveMapping ?? const {},
        );
        _loading = false;
      });
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: switchmap load failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() {
        _loading = false;
        _error = 'Switch map load failed: $error';
      });
    }
  }

  void _selectType(String typeId) {
    if (typeId == _typeId) return;
    final type = _load.types[typeId];
    if (type == null) return;
    setState(() {
      _typeId = typeId;
      _draftMappings = Map<String, Object?>.from(type.effectiveMapping);
      _error = null;
    });
  }

  void _setAction(String eventKey, String value) {
    if (!_modernExpertCanEditSwitchmap) return;
    final action = value == '__none__' ? null : value;
    final current = _draftMappings[eventKey];
    final next = _isAdjustmentAction(action)
        ? {
            'action': action,
            'when_off': _mappingWhenOff(current),
          }
        : action;
    setState(() => _draftMappings = {..._draftMappings, eventKey: next});
  }

  void _setWhenOff(String eventKey, String value) {
    if (!_modernExpertCanEditSwitchmap) return;
    final current = _draftMappings[eventKey];
    final action = _mappingMainAction(current);
    setState(() {
      _draftMappings = {
        ..._draftMappings,
        eventKey: {
          'action': action,
          'when_off': value == '__none__' ? null : value,
        },
      };
    });
  }

  void _resetTypeToDefaults() {
    if (!_modernExpertCanEditSwitchmap) return;
    final typeId = _typeId;
    if (typeId == null) return;
    final type = _load.types[typeId];
    if (type == null) return;
    HapticFeedback.selectionClick();
    setState(() {
      _draftMappings = Map<String, Object?>.from(type.defaultMapping);
    });
  }

  void _cancelChanges() {
    final typeId = _typeId;
    if (typeId == null) return;
    final type = _load.types[typeId];
    if (type == null) return;
    HapticFeedback.selectionClick();
    setState(() {
      _draftMappings = Map<String, Object?>.from(type.effectiveMapping);
      _error = null;
    });
  }

  Future<void> _saveMappings() async {
    if (!_modernExpertCanEditSwitchmap) {
      setState(
        () => _error = 'This build is missing the Expert switch map API.',
      );
      return;
    }
    final client = widget.client;
    final typeId = _typeId;
    if (client == null || typeId == null) return;
    final type = _load.types[typeId];
    if (type == null) return;

    final customForType = <String, Object?>{};
    for (final entry in _draftMappings.entries) {
      if (!_mappingEquals(entry.value, type.defaultMapping[entry.key])) {
        customForType[entry.key] = _mappingToServer(entry.value);
      }
    }

    final nextCustom = <String, Map<String, Object?>>{
      for (final entry in _load.customMappings.entries)
        entry.key: Map<String, Object?>.from(entry.value),
    };
    if (customForType.isEmpty) {
      nextCustom.remove(typeId);
    } else {
      nextCustom[typeId] = customForType;
    }

    HapticFeedback.selectionClick();
    setState(() {
      _saving = true;
      _error = null;
    });
    try {
      await client.saveSwitchmapCustomMappings(nextCustom);
      await _loadSwitchmap(keepTypeId: typeId);
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(content: Text('Switch mappings saved')),
      );
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: switchmap save failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() => _error = 'Switch map save failed: $error');
    } finally {
      if (mounted) setState(() => _saving = false);
    }
  }

  bool get _hasChanges {
    final typeId = _typeId;
    if (typeId == null) return false;
    final type = _load.types[typeId];
    if (type == null) return false;
    final keys = {..._draftMappings.keys, ...type.effectiveMapping.keys};
    for (final key in keys) {
      if (!_mappingEquals(_draftMappings[key], type.effectiveMapping[key])) {
        return true;
      }
    }
    return false;
  }

  List<String> _displayActionTypes(_ExpertSwitchmapType type) {
    final supported = type.actionTypes;
    final result = <String>[];
    if (supported.contains('short_release')) {
      result.add('short_release');
    } else if (supported.contains('press')) {
      result.add('press');
    }
    for (final action in [
      'double_press',
      'triple_press',
      'quadruple_press',
      'quintuple_press',
      'hold',
      'rotate',
    ]) {
      if (supported.contains(action)) result.add(action);
    }
    return result;
  }

  List<String> _actionItems(String actionType) {
    final dialOnly = actionType == 'rotate';
    final items = <String>[];
    for (final category in _load.actionCategories.entries) {
      if (dialOnly && category.key != 'Dial') continue;
      if (!dialOnly && category.key == 'Dial') continue;
      for (final action in category.value) {
        final id = action.id ?? '__none__';
        if (!items.contains(id)) items.add(id);
      }
    }
    if (!items.contains('__none__')) items.insert(0, '__none__');
    return items;
  }

  String _actionLabel(String id) {
    if (id == '__none__') return '-';
    for (final actions in _load.actionCategories.values) {
      for (final action in actions) {
        if (action.id == id) return action.label;
      }
    }
    return _titleCase(id);
  }

  List<String> get _whenOffItems => [
        for (final option in _load.whenOffOptions) option.id ?? '__none__',
      ];

  String _whenOffLabel(String id) {
    if (id == '__none__') return '-';
    for (final option in _load.whenOffOptions) {
      if (option.id == id) return option.label;
    }
    return _titleCase(id);
  }

  @override
  Widget build(BuildContext context) {
    final typeId = _typeId;
    final type = typeId == null ? null : _load.types[typeId];
    final typeItems = _load.types.keys.toList()
      ..sort((a, b) => _load.types[a]!.name.compareTo(_load.types[b]!.name));

    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            _ExpertHeader(
              title: 'Switch map',
              leading:
                  _BackButton(onPressed: () => Navigator.of(context).pop()),
              trailing: IconButton(
                onPressed: _loading ? null : () => _loadSwitchmap(),
                tooltip: 'Refresh switch map',
                icon: const Icon(Icons.refresh_rounded),
                color: CelestialColors.textSecondary,
                style: IconButton.styleFrom(
                  backgroundColor: _panel2Color,
                  fixedSize: const Size(40, 40),
                  minimumSize: const Size(40, 40),
                  shape: RoundedRectangleBorder(
                    borderRadius: BorderRadius.circular(8),
                  ),
                  side: const BorderSide(color: CelestialColors.orbitRing),
                ),
              ),
            ),
            Expanded(
              child: ListView(
                padding: const EdgeInsets.fromLTRB(16, 0, 16, 28),
                children: [
                  _ExpertPanel(
                    padding: const EdgeInsets.all(12),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        Row(
                          children: [
                            Expanded(
                              child: typeItems.isEmpty
                                  ? const Text(
                                      'No switch types available.',
                                      style: TextStyle(
                                        color: CelestialColors.textSecondary,
                                        fontSize: 13,
                                      ),
                                    )
                                  : _CompactDropdown<String>(
                                      value: typeId ?? typeItems.first,
                                      items: typeItems,
                                      itemLabel: (id) =>
                                          _load.types[id]?.name ?? id,
                                      onChanged: _selectType,
                                    ),
                            ),
                            if (_loading || _saving) ...[
                              const SizedBox(width: 10),
                              const SizedBox(
                                width: 18,
                                height: 18,
                                child:
                                    CircularProgressIndicator(strokeWidth: 2),
                              ),
                            ],
                          ],
                        ),
                        if (type != null) ...[
                          const SizedBox(height: 10),
                          Text(
                            type.hasCustom
                                ? 'Custom defaults active for this switch type.'
                                : 'Using built-in defaults for this switch type.',
                            style: const TextStyle(
                              color: CelestialColors.textSecondary,
                              fontSize: 12,
                            ),
                          ),
                        ],
                        if (_error != null) ...[
                          const SizedBox(height: 10),
                          Text(
                            _error!,
                            style: const TextStyle(
                              color: _dangerAccent,
                              fontSize: 12,
                              fontWeight: FontWeight.w600,
                            ),
                          ),
                        ],
                      ],
                    ),
                  ),
                  const SizedBox(height: 12),
                  if (type == null)
                    const _ExpertPanel(
                      child: Text(
                        'No switch map data loaded.',
                        style: TextStyle(
                          color: CelestialColors.textSecondary,
                          fontSize: 13,
                        ),
                      ),
                    )
                  else ...[
                    for (final button in type.buttons)
                      _SwitchmapButtonCard(
                        button: button,
                        actionTypes: _displayActionTypes(type),
                        draftMappings: _draftMappings,
                        actionItems: _actionItems,
                        actionLabel: _actionLabel,
                        whenOffItems: _whenOffItems,
                        whenOffLabel: _whenOffLabel,
                        enabled: _modernExpertCanEditSwitchmap,
                        onActionChanged: _setAction,
                        onWhenOffChanged: _setWhenOff,
                      ),
                    _ExpertPanel(
                      padding: const EdgeInsets.all(12),
                      child: Row(
                        children: [
                          Expanded(
                            child: Text(
                              _hasChanges
                                  ? 'Unsaved switch map changes'
                                  : 'Switch map matches saved config',
                              style: TextStyle(
                                color: _hasChanges
                                    ? _expertAccent
                                    : CelestialColors.textSecondary,
                                fontSize: 12,
                                fontWeight: FontWeight.w700,
                              ),
                            ),
                          ),
                          TextButton(
                            onPressed: _saving || !_modernExpertCanEditSwitchmap
                                ? null
                                : _cancelChanges,
                            child: const Text('Cancel'),
                          ),
                          TextButton(
                            onPressed: _saving || !_modernExpertCanEditSwitchmap
                                ? null
                                : _resetTypeToDefaults,
                            child: const Text('Defaults'),
                          ),
                          FilledButton(
                            onPressed: _hasChanges &&
                                    !_saving &&
                                    _modernExpertCanEditSwitchmap
                                ? _saveMappings
                                : null,
                            style: FilledButton.styleFrom(
                              backgroundColor: _expertAccent,
                              foregroundColor: Colors.black,
                            ),
                            child: const Text('Save'),
                          ),
                        ],
                      ),
                    ),
                  ],
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _ExpertControlDetailScreen extends StatefulWidget {
  final _CircadianExpertClient? client;
  final _ExpertControl initialControl;
  final List<_ExpertZone> zones;
  final int defaultPauseMinutes;

  const _ExpertControlDetailScreen({
    required this.client,
    required this.initialControl,
    required this.zones,
    required this.defaultPauseMinutes,
  });

  @override
  State<_ExpertControlDetailScreen> createState() =>
      _ExpertControlDetailScreenState();
}

class _ExpertControlDetailScreenState
    extends State<_ExpertControlDetailScreen> {
  late _ExpertControl _control = widget.initialControl;
  List<_ExpertActivityEntry> _activityEntries = const [];
  _ExpertZhaSettings? _zhaSettings;
  bool _activityLoading = true;
  bool _zhaLoading = false;
  bool _saving = false;
  String? _savingKey;
  String? _zhaSavingKey;
  String? _error;
  final Map<int, String?> _areaPickerByScope = {};
  final Map<int, String?> _sectionPickerByScope = {};

  @override
  void initState() {
    super.initState();
    unawaited(_loadControlActivity());
    unawaited(_loadZhaSettings());
  }

  List<_ExpertArea> get _areas => [
        for (final zone in widget.zones) ...zone.areas,
      ];

  List<_ExpertReachSection> get _sections => _reachSectionsForAreas(_areas);

  String _areaName(String areaId) {
    for (final area in _areas) {
      if (area.id == areaId) return area.name;
    }
    return areaId;
  }

  String _sectionName(String sectionId) {
    return _sectionReachLabel(_areas, sectionId);
  }

  Future<void> _loadControlActivity() async {
    final client = widget.client;
    if (client == null) {
      setState(() => _activityLoading = false);
      return;
    }
    setState(() => _activityLoading = true);
    try {
      final entries = await client.fetchControlActivity(_control);
      if (!mounted) return;
      setState(() {
        _activityEntries = entries;
        _activityLoading = false;
      });
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: control activity load failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() => _activityLoading = false);
    }
  }

  bool get _supportsZhaSettings =>
      _control.category == 'motion_sensor' &&
      _control.integration == 'zha' &&
      _control.deviceId != null;

  Future<void> _loadZhaSettings() async {
    final client = widget.client;
    final deviceId = _control.deviceId;
    if (client == null || deviceId == null || !_supportsZhaSettings) {
      if (mounted) {
        setState(() {
          _zhaSettings = null;
          _zhaLoading = false;
        });
      }
      return;
    }

    setState(() => _zhaLoading = true);
    try {
      final settings = await client.fetchZhaSettings(deviceId);
      if (!mounted) return;
      setState(() {
        _zhaSettings = settings;
        _zhaLoading = false;
      });
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: ZHA settings load failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() {
        _zhaSettings = null;
        _zhaLoading = false;
      });
    }
  }

  Future<void> _saveZhaSettings({
    String? sensitivity,
    int? timeoutSeconds,
  }) async {
    final client = widget.client;
    final deviceId = _control.deviceId;
    if (client == null || deviceId == null) {
      _showSnack('Connect to RhythmOS before changing ZHA settings.');
      return;
    }

    final savingKey = sensitivity != null ? 'sensitivity' : 'timeout';
    HapticFeedback.selectionClick();
    setState(() => _zhaSavingKey = savingKey);
    try {
      await client.saveZhaSettings(
        deviceId,
        sensitivity: sensitivity,
        timeoutSeconds: timeoutSeconds,
      );
      await _loadZhaSettings();
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: ZHA settings save failed: $error');
      debugPrint('$stackTrace');
      _showSnack('ZHA settings save failed: $error');
    } finally {
      if (mounted) setState(() => _zhaSavingKey = null);
    }
  }

  Future<void> _saveControl(
    _ExpertControl next, {
    required String savingKey,
  }) async {
    if (!_modernExpertCanEditInputBindings) {
      _showSnack('This build is missing the Expert control reach API.');
      return;
    }
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before saving this control.');
      return;
    }

    final previous = _control;
    HapticFeedback.selectionClick();
    setState(() {
      _control = next;
      _saving = true;
      _savingKey = savingKey;
      _error = null;
    });
    try {
      await client.configureControl(next);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: control save failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() {
        _control = previous;
        _error = 'Control save failed: $error';
      });
      _showSnack('Control save failed: $error');
    } finally {
      if (mounted) {
        setState(() {
          _saving = false;
          _savingKey = null;
        });
      }
    }
  }

  Future<void> _setPaused(bool paused) async {
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before saving this control.');
      return;
    }
    final inactiveUntil =
        paused ? _pauseUntilIso(widget.defaultPauseMinutes) : null;
    final previous = _control;
    HapticFeedback.selectionClick();
    setState(() {
      _control = _control.copyWith(
        inactive: paused,
        inactiveUntil: inactiveUntil,
        status: _controlStatusWithPause(_control.status, paused),
      );
      _saving = true;
      _savingKey = 'pause';
      _error = null;
    });
    try {
      await client.setControlPause(
        _control.pauseEndpointKey,
        inactive: paused,
        inactiveUntil: inactiveUntil,
      );
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: control pause save failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() {
        _control = previous;
        _error = 'Pause save failed: $error';
      });
      _showSnack('Pause save failed: $error');
    } finally {
      if (mounted) {
        setState(() {
          _saving = false;
          _savingKey = null;
        });
      }
    }
  }

  Future<void> _addScope() async {
    if (_control.scopes.length >= _expertMaxReaches) {
      _showSnack('Maximum $_expertMaxReaches reaches allowed.');
      return;
    }
    final scopes = [..._control.scopes, const _ExpertSwitchScope.empty()];
    await _saveControl(
      _control.copyWith(scopes: scopes),
      savingKey: 'scope:add',
    );
  }

  Future<void> _removeScope(int index) async {
    if (index < 0 || index >= _control.scopes.length) return;
    if (_control.scopes.length <= 1) {
      _showSnack('Keep at least one reach configured.');
      return;
    }
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: Text(
          'Remove Reach ${index + 1}?',
          style: const TextStyle(color: CelestialColors.textPrimary),
        ),
        content: const Text(
          'This removes the reach and its schedule/action settings.',
          style: TextStyle(color: CelestialColors.textSecondary),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('Cancel'),
          ),
          TextButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text(
              'Remove',
              style: TextStyle(color: _dangerAccent),
            ),
          ),
        ],
      ),
    );
    if (!mounted || confirmed != true) return;
    final scopes = List<_ExpertSwitchScope>.from(_control.scopes)
      ..removeAt(index);
    await _saveControl(
      _control.copyWith(scopes: scopes),
      savingKey: 'scope:$index',
    );
  }

  Future<void> _moveScope(int index, int direction) async {
    if (index < 0 || index >= _control.scopes.length) return;
    final targetIndex = index + direction;
    if (targetIndex < 0 || targetIndex >= _control.scopes.length) return;
    final scopes = List<_ExpertSwitchScope>.from(_control.scopes);
    final scope = scopes.removeAt(index);
    scopes.insert(targetIndex, scope);
    await _saveControl(
      _control.copyWith(scopes: scopes),
      savingKey: 'scope:$targetIndex',
    );
  }

  Future<void> _updateScope(int index, _ExpertSwitchScope scope) async {
    if (index < 0 || index >= _control.scopes.length) return;
    final scopes = List<_ExpertSwitchScope>.from(_control.scopes);
    scopes[index] = scope;
    await _saveControl(
      _control.copyWith(scopes: scopes),
      savingKey: 'scope:$index',
    );
  }

  Future<void> _addAreaToScope(int index, String areaId) async {
    if (index < 0 || index >= _control.scopes.length) return;
    final scope = _control.scopes[index];
    if (scope.areaIds.contains(areaId)) return;
    await _updateScope(
      index,
      _scopeWithAreaIds(scope, [...scope.areaIds, areaId]),
    );
  }

  Future<void> _removeAreaFromScope(int index, String areaId) async {
    if (index < 0 || index >= _control.scopes.length) return;
    final scope = _control.scopes[index];
    await _updateScope(
      index,
      _scopeWithAreaIds(
        scope,
        scope.areaIds.where((id) => id != areaId),
      ),
    );
  }

  Future<void> _addSectionToScope(int index, String sectionId) async {
    if (index < 0 || index >= _control.scopes.length) return;
    final scope = _control.scopes[index];
    if (scope.sectionIds.contains(sectionId)) return;
    await _updateScope(
      index,
      _scopeWithSectionIds(scope, [...scope.sectionIds, sectionId]),
    );
  }

  Future<void> _removeSectionFromScope(int index, String sectionId) async {
    if (index < 0 || index >= _control.scopes.length) return;
    final scope = _control.scopes[index];
    await _updateScope(
      index,
      _scopeWithSectionIds(
        scope,
        scope.sectionIds.where((id) => id != sectionId),
      ),
    );
  }

  Future<void> _resetControl() async {
    if (!_modernExpertCanEditInputBindings) {
      _showSnack('This build is missing the Expert control reset API.');
      return;
    }
    final client = widget.client;
    if (client == null) return;
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: const Text(
          'Reset control',
          style: TextStyle(color: CelestialColors.textPrimary),
        ),
        content: Text(
          'Clear configuration for "${_control.name}"? The control stays in the list as setup needed.',
          style: const TextStyle(color: CelestialColors.textSecondary),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('Cancel'),
          ),
          TextButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text(
              'Reset',
              style: TextStyle(color: _dangerAccent),
            ),
          ),
        ],
      ),
    );
    if (confirmed != true) return;

    HapticFeedback.selectionClick();
    setState(() {
      _saving = true;
      _savingKey = 'reset';
    });
    try {
      await client.resetControlConfig(_control.id);
      if (mounted) Navigator.of(context).pop();
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: control reset failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() => _error = 'Reset failed: $error');
      _showSnack('Reset failed: $error');
    } finally {
      if (mounted) {
        setState(() {
          _saving = false;
          _savingKey = null;
        });
      }
    }
  }

  void _showSnack(String message) {
    if (!mounted) return;
    ScaffoldMessenger.of(context)
        .showSnackBar(SnackBar(content: Text(message)));
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            _ExpertHeader(
              title: 'Control',
              leading:
                  _BackButton(onPressed: () => Navigator.of(context).pop()),
              trailing: IconButton(
                onPressed: _saving || !_modernExpertCanEditInputBindings
                    ? null
                    : _resetControl,
                tooltip: 'Reset control',
                icon: const Icon(Icons.restart_alt_rounded),
                color: _dangerAccent,
                style: IconButton.styleFrom(
                  backgroundColor: _panel2Color,
                  fixedSize: const Size(40, 40),
                  minimumSize: const Size(40, 40),
                  shape: RoundedRectangleBorder(
                    borderRadius: BorderRadius.circular(8),
                  ),
                  side: const BorderSide(color: CelestialColors.orbitRing),
                ),
              ),
            ),
            Expanded(
              child: ListView(
                padding: const EdgeInsets.fromLTRB(16, 0, 16, 28),
                children: [
                  _ExpertPanel(
                    padding: const EdgeInsets.all(14),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Row(
                          children: [
                            Container(
                              width: 38,
                              height: 38,
                              decoration: BoxDecoration(
                                color: _changedAccent.withValues(alpha: 0.12),
                                borderRadius: BorderRadius.circular(8),
                                border: Border.all(
                                  color: _changedAccent.withValues(alpha: 0.4),
                                ),
                              ),
                              child: Icon(
                                _controlCategoryIcon(_control.category),
                                color: _changedAccent,
                                size: 20,
                              ),
                            ),
                            const SizedBox(width: 10),
                            Expanded(
                              child: Column(
                                crossAxisAlignment: CrossAxisAlignment.start,
                                children: [
                                  Text(
                                    _control.name,
                                    maxLines: 2,
                                    overflow: TextOverflow.ellipsis,
                                    style: const TextStyle(
                                      color: CelestialColors.textPrimary,
                                      fontSize: 20,
                                      fontWeight: FontWeight.w800,
                                    ),
                                  ),
                                  Text(
                                    _controlSubtitle(_control),
                                    maxLines: 2,
                                    overflow: TextOverflow.ellipsis,
                                    style: const TextStyle(
                                      color: CelestialColors.textSecondary,
                                      fontSize: 12,
                                      height: 1.3,
                                    ),
                                  ),
                                ],
                              ),
                            ),
                            if (_saving)
                              const SizedBox(
                                width: 18,
                                height: 18,
                                child:
                                    CircularProgressIndicator(strokeWidth: 2),
                              ),
                          ],
                        ),
                        if (_control.deviceId != null) ...[
                          const SizedBox(height: 10),
                          Text(
                            'Device: ${_control.deviceId}',
                            style: const TextStyle(
                              color: CelestialColors.textSecondary,
                              fontSize: 11,
                            ),
                          ),
                        ],
                        if (_error != null) ...[
                          const SizedBox(height: 8),
                          Text(
                            _error!,
                            style: const TextStyle(
                              color: _dangerAccent,
                              fontSize: 12,
                              fontWeight: FontWeight.w600,
                            ),
                          ),
                        ],
                      ],
                    ),
                  ),
                  const SizedBox(height: 12),
                  _ExpertPanel(
                    child: _ConfigToggleRow(
                      label: _control.inactive
                          ? 'Paused ${_pauseUntilText(_control.inactiveUntil)}'
                          : 'Pause control',
                      value: _control.inactive,
                      saving: _savingKey == 'pause',
                      onChanged: (value) => unawaited(_setPaused(value)),
                    ),
                  ),
                  const SizedBox(height: 12),
                  if (_supportsZhaSettings &&
                      (_zhaLoading ||
                          (_zhaSettings?.hasControls ?? false))) ...[
                    _ExpertPanel(
                      child: _ZhaSettingsPanel(
                        settings: _zhaSettings,
                        loading: _zhaLoading,
                        savingKey: _zhaSavingKey,
                        onSensitivityChanged: (value) => unawaited(
                          _saveZhaSettings(sensitivity: value),
                        ),
                        onTimeoutChanged: (value) => unawaited(
                          _saveZhaSettings(timeoutSeconds: value),
                        ),
                      ),
                    ),
                    const SizedBox(height: 12),
                  ],
                  _ExpertPanel(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        Row(
                          children: [
                            const Text(
                              'Reaches',
                              style: TextStyle(
                                color: CelestialColors.textPrimary,
                                fontSize: 15,
                                fontWeight: FontWeight.w800,
                              ),
                            ),
                            const Spacer(),
                            OutlinedButton.icon(
                              onPressed:
                                  _saving || !_modernExpertCanEditInputBindings
                                      ? null
                                      : _addScope,
                              icon: const Icon(Icons.add_rounded, size: 17),
                              label: const Text('Reach'),
                              style: OutlinedButton.styleFrom(
                                foregroundColor: _expertAccent,
                                side: const BorderSide(
                                  color: CelestialColors.orbitRing,
                                ),
                                shape: RoundedRectangleBorder(
                                  borderRadius: BorderRadius.circular(8),
                                ),
                              ),
                            ),
                          ],
                        ),
                        const SizedBox(height: 10),
                        if (_control.scopes.isEmpty)
                          const Text(
                            'No reaches configured.',
                            style: TextStyle(
                              color: CelestialColors.textSecondary,
                              fontSize: 12,
                            ),
                          )
                        else
                          for (int i = 0; i < _control.scopes.length; i++)
                            _ControlScopeCard(
                              index: i,
                              scope: _control.scopes[i],
                              areas: _areas,
                              sections: _sections,
                              triggerEntities: _control.binarySensors,
                              areaName: _areaName,
                              sectionName: _sectionName,
                              saving: _savingKey == 'scope:$i',
                              enabled: _modernExpertCanEditInputBindings,
                              selectedAreaId: _areaPickerByScope[i],
                              onSelectedAreaChanged: (value) => setState(
                                () => _areaPickerByScope[i] = value,
                              ),
                              selectedSectionId: _sectionPickerByScope[i],
                              onSelectedSectionChanged: (value) => setState(
                                () => _sectionPickerByScope[i] = value,
                              ),
                              onAddArea: (areaId) =>
                                  unawaited(_addAreaToScope(i, areaId)),
                              onRemoveArea: (areaId) =>
                                  unawaited(_removeAreaFromScope(i, areaId)),
                              onAddSection: (sectionId) =>
                                  unawaited(_addSectionToScope(i, sectionId)),
                              onRemoveSection: (sectionId) => unawaited(
                                _removeSectionFromScope(i, sectionId),
                              ),
                              onUpdate: (scope) =>
                                  unawaited(_updateScope(i, scope)),
                              canMoveUp: i > 0,
                              canMoveDown: i < _control.scopes.length - 1,
                              onMoveUp: () => unawaited(_moveScope(i, -1)),
                              onMoveDown: () => unawaited(_moveScope(i, 1)),
                              canRemoveScope: _control.scopes.length > 1,
                              onRemoveScope: () => unawaited(_removeScope(i)),
                            ),
                      ],
                    ),
                  ),
                  if (_activityLoading || _activityEntries.isNotEmpty) ...[
                    const SizedBox(height: 12),
                    _ExpertPanel(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: [
                          Row(
                            children: [
                              const Text(
                                'Activity',
                                style: TextStyle(
                                  color: CelestialColors.textPrimary,
                                  fontSize: 15,
                                  fontWeight: FontWeight.w800,
                                ),
                              ),
                              const Spacer(),
                              if (_activityLoading)
                                const SizedBox(
                                  width: 16,
                                  height: 16,
                                  child: CircularProgressIndicator(
                                    strokeWidth: 2,
                                  ),
                                )
                              else
                                Text(
                                  '${_activityEntries.length}',
                                  style: const TextStyle(
                                    color: CelestialColors.textSecondary,
                                    fontSize: 12,
                                    fontWeight: FontWeight.w700,
                                  ),
                                ),
                              IconButton(
                                onPressed: _activityLoading
                                    ? null
                                    : _loadControlActivity,
                                tooltip: 'Refresh activity',
                                icon: const Icon(Icons.refresh_rounded),
                                color: CelestialColors.textSecondary,
                                visualDensity: VisualDensity.compact,
                              ),
                            ],
                          ),
                          const SizedBox(height: 8),
                          if (_activityEntries.isEmpty)
                            const Text(
                              'No activity yet for this control.',
                              style: TextStyle(
                                color: CelestialColors.textSecondary,
                                fontSize: 12,
                              ),
                            )
                          else
                            for (final entry in _activityEntries.take(8))
                              _ActivityEntryRow(
                                entry: entry,
                                areaName: entry.areaId == null
                                    ? null
                                    : _areaName(entry.areaId!),
                              ),
                        ],
                      ),
                    ),
                  ],
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _ExpertSwitchDetailScreen extends StatefulWidget {
  final _CircadianExpertClient? client;
  final _ExpertSwitch initialSwitch;
  final Map<String, _ExpertSwitchType> switchTypes;
  final List<_ExpertMoment> moments;
  final List<_ExpertZone> zones;

  const _ExpertSwitchDetailScreen({
    required this.client,
    required this.initialSwitch,
    required this.switchTypes,
    required this.moments,
    required this.zones,
  });

  @override
  State<_ExpertSwitchDetailScreen> createState() =>
      _ExpertSwitchDetailScreenState();
}

class _ExpertSwitchDetailScreenState extends State<_ExpertSwitchDetailScreen> {
  late _ExpertSwitch _control = widget.initialSwitch;
  bool _saving = false;
  String? _savingKey;
  String? _error;
  final Map<int, String?> _areaPickerByScope = {};
  final Map<int, String?> _sectionPickerByScope = {};

  List<_ExpertArea> get _areas => [
        for (final zone in widget.zones) ...zone.areas,
      ];

  List<_ExpertReachSection> get _sections => _reachSectionsForAreas(_areas);

  String _areaName(String areaId) {
    for (final area in _areas) {
      if (area.id == areaId) return area.name;
    }
    return areaId;
  }

  String _sectionName(String sectionId) {
    return _sectionReachLabel(_areas, sectionId);
  }

  _ExpertSwitchType? get _type => widget.switchTypes[_control.type];

  List<String> get _buttonEvents {
    final events = <String>{..._control.magicButtons.keys};
    final type = _type;
    if (type != null) {
      events.addAll(type.defaultMapping.keys);
      if (events.isEmpty) {
        for (final button in type.buttons) {
          for (final actionType in type.actionTypes.take(4)) {
            events.add('${button}_$actionType');
          }
        }
      }
    }
    final list = events.toList()..sort(_compareButtonEvents);
    return list;
  }

  Future<void> _saveSwitch(
    _ExpertSwitch next, {
    required String savingKey,
  }) async {
    if (!_modernExpertCanEditInputBindings) {
      _showSnack('This build is missing the Expert switch editing API.');
      return;
    }
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before saving this switch.');
      return;
    }

    final previous = _control;
    HapticFeedback.selectionClick();
    setState(() {
      _control = next;
      _saving = true;
      _savingKey = savingKey;
      _error = null;
    });
    try {
      await client.updateSwitch(next);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: switch save failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() {
        _control = previous;
        _error = 'Switch save failed: $error';
      });
      _showSnack('Switch save failed: $error');
    } finally {
      if (mounted) {
        setState(() {
          _saving = false;
          _savingKey = null;
        });
      }
    }
  }

  Future<void> _renameSwitch() async {
    final name = await _promptText(
      title: 'Rename switch',
      label: 'Switch name',
      initialValue: _control.name,
    );
    if (name == null || name == _control.name) return;
    await _saveSwitch(
      _control.copyWith(name: name),
      savingKey: 'name',
    );
  }

  Future<void> _setType(String type) async {
    if (type == _control.type) return;
    final typeName = widget.switchTypes[type]?.name ?? type;
    await _saveSwitch(
      _control.copyWith(type: type, typeName: typeName),
      savingKey: 'type',
    );
  }

  Future<void> _setMagicButton(String event, String value) async {
    final nextMagic = Map<String, String?>.from(_control.magicButtons);
    if (value == '__default__') {
      nextMagic.remove(event);
    } else if (value == '__custom__') {
      return;
    } else {
      nextMagic[event] = 'set_$value';
    }
    await _saveSwitch(
      _control.copyWith(magicButtons: nextMagic),
      savingKey: 'magic:$event',
    );
  }

  Future<void> _addScope() async {
    if (_control.scopes.length >= _expertMaxReaches) {
      _showSnack('Maximum $_expertMaxReaches reaches allowed.');
      return;
    }
    final scopes = [..._control.scopes, const _ExpertSwitchScope.empty()];
    await _saveSwitch(
      _control.copyWith(scopes: scopes),
      savingKey: 'scope:add',
    );
  }

  Future<void> _removeScope(int index) async {
    if (index < 0 || index >= _control.scopes.length) return;
    if (_control.scopes.length <= 1) {
      _showSnack('Keep at least one reach configured.');
      return;
    }
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: Text(
          'Remove Reach ${index + 1}?',
          style: const TextStyle(color: CelestialColors.textPrimary),
        ),
        content: const Text(
          'This removes the reach and its button assignments for that target group.',
          style: TextStyle(color: CelestialColors.textSecondary),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('Cancel'),
          ),
          TextButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text(
              'Remove',
              style: TextStyle(color: _dangerAccent),
            ),
          ),
        ],
      ),
    );
    if (!mounted || confirmed != true) return;
    final scopes = List<_ExpertSwitchScope>.from(_control.scopes)
      ..removeAt(index);
    await _saveSwitch(
      _control.copyWith(scopes: scopes),
      savingKey: 'scope:$index',
    );
  }

  Future<void> _moveScope(int index, int direction) async {
    if (index < 0 || index >= _control.scopes.length) return;
    final targetIndex = index + direction;
    if (targetIndex < 0 || targetIndex >= _control.scopes.length) return;
    final scopes = List<_ExpertSwitchScope>.from(_control.scopes);
    final scope = scopes.removeAt(index);
    scopes.insert(targetIndex, scope);
    await _saveSwitch(
      _control.copyWith(scopes: scopes),
      savingKey: 'scope:$targetIndex',
    );
  }

  Future<void> _updateScope(int index, _ExpertSwitchScope scope) async {
    if (index < 0 || index >= _control.scopes.length) return;
    final scopes = List<_ExpertSwitchScope>.from(_control.scopes);
    scopes[index] = scope;
    await _saveSwitch(
      _control.copyWith(scopes: scopes),
      savingKey: 'scope:$index',
    );
  }

  Future<void> _addAreaToScope(int index, String areaId) async {
    if (index < 0 || index >= _control.scopes.length) return;
    final scope = _control.scopes[index];
    if (scope.areaIds.contains(areaId)) return;
    await _updateScope(
      index,
      _scopeWithAreaIds(scope, [...scope.areaIds, areaId]),
    );
  }

  Future<void> _removeAreaFromScope(int index, String areaId) async {
    if (index < 0 || index >= _control.scopes.length) return;
    final scope = _control.scopes[index];
    await _updateScope(
      index,
      _scopeWithAreaIds(
        scope,
        scope.areaIds.where((id) => id != areaId),
      ),
    );
  }

  Future<void> _addSectionToScope(int index, String sectionId) async {
    if (index < 0 || index >= _control.scopes.length) return;
    final scope = _control.scopes[index];
    if (scope.sectionIds.contains(sectionId)) return;
    await _updateScope(
      index,
      _scopeWithSectionIds(scope, [...scope.sectionIds, sectionId]),
    );
  }

  Future<void> _removeSectionFromScope(int index, String sectionId) async {
    if (index < 0 || index >= _control.scopes.length) return;
    final scope = _control.scopes[index];
    await _updateScope(
      index,
      _scopeWithSectionIds(
        scope,
        scope.sectionIds.where((id) => id != sectionId),
      ),
    );
  }

  Future<String?> _promptText({
    required String title,
    required String label,
    required String initialValue,
  }) async {
    final controller = TextEditingController(text: initialValue);
    final value = await showDialog<String>(
      context: context,
      builder: (context) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: Text(
          title,
          style: const TextStyle(color: CelestialColors.textPrimary),
        ),
        content: TextField(
          controller: controller,
          autofocus: true,
          style: const TextStyle(color: CelestialColors.textPrimary),
          decoration: InputDecoration(
            labelText: label,
            labelStyle: const TextStyle(color: CelestialColors.textSecondary),
            enabledBorder: const UnderlineInputBorder(
              borderSide: BorderSide(color: CelestialColors.orbitRing),
            ),
            focusedBorder: const UnderlineInputBorder(
              borderSide: BorderSide(color: _expertAccent),
            ),
          ),
          onSubmitted: (value) => Navigator.of(context).pop(value),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(),
            child: const Text('Cancel'),
          ),
          TextButton(
            onPressed: () => Navigator.of(context).pop(controller.text),
            child: const Text('Save'),
          ),
        ],
      ),
    );
    controller.dispose();
    final trimmed = value?.trim();
    return trimmed == null || trimmed.isEmpty ? null : trimmed;
  }

  String _magicDropdownValue(String event) {
    final action = _control.magicButtons[event];
    if (action == null || action.isEmpty) return '__default__';
    if (action.startsWith('set_')) {
      final momentId = action.substring(4);
      if (widget.moments.any((moment) => moment.id == momentId)) {
        return momentId;
      }
    }
    return '__custom__';
  }

  List<String> _magicItems(String event) {
    final current = _magicDropdownValue(event);
    return [
      '__default__',
      if (current == '__custom__') '__custom__',
      for (final moment in widget.moments) moment.id,
    ];
  }

  String _magicItemLabel(String id) {
    if (id == '__default__') return 'Default action';
    if (id == '__custom__') return 'Custom action';
    for (final moment in widget.moments) {
      if (moment.id == id) return moment.name;
    }
    return id;
  }

  void _showSnack(String message) {
    if (!mounted) return;
    ScaffoldMessenger.of(context)
        .showSnackBar(SnackBar(content: Text(message)));
  }

  @override
  Widget build(BuildContext context) {
    final typeItems = _optionsWithCurrent(
      _control.type,
      widget.switchTypes.keys.toList()..sort(),
    );
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            _ExpertHeader(
              title: 'Switch',
              leading:
                  _BackButton(onPressed: () => Navigator.of(context).pop()),
              trailing: IconButton(
                onPressed: _saving || !_modernExpertCanEditInputBindingIdentity
                    ? null
                    : _renameSwitch,
                tooltip: 'Rename switch',
                icon: const Icon(Icons.edit_rounded),
                color: _expertAccent,
                style: IconButton.styleFrom(
                  backgroundColor: _panel2Color,
                  fixedSize: const Size(40, 40),
                  minimumSize: const Size(40, 40),
                  shape: RoundedRectangleBorder(
                    borderRadius: BorderRadius.circular(8),
                  ),
                  side: const BorderSide(color: CelestialColors.orbitRing),
                ),
              ),
            ),
            Expanded(
              child: ListView(
                padding: const EdgeInsets.fromLTRB(16, 0, 16, 28),
                children: [
                  _ExpertPanel(
                    padding: const EdgeInsets.all(14),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Row(
                          children: [
                            Expanded(
                              child: Text(
                                _control.name,
                                maxLines: 2,
                                overflow: TextOverflow.ellipsis,
                                style: const TextStyle(
                                  color: CelestialColors.textPrimary,
                                  fontSize: 21,
                                  fontWeight: FontWeight.w800,
                                ),
                              ),
                            ),
                            if (_saving)
                              const SizedBox(
                                width: 18,
                                height: 18,
                                child:
                                    CircularProgressIndicator(strokeWidth: 2),
                              ),
                          ],
                        ),
                        const SizedBox(height: 4),
                        Text(
                          _switchSubtitle(_control),
                          style: const TextStyle(
                            color: CelestialColors.textSecondary,
                            fontSize: 12,
                            height: 1.3,
                          ),
                        ),
                        const SizedBox(height: 12),
                        _MomentFieldRow(
                          label: 'Type',
                          child: _CompactDropdown<String>(
                            value: _control.type,
                            items: typeItems,
                            itemLabel: (id) =>
                                widget.switchTypes[id]?.name ?? id,
                            enabled: _modernExpertCanEditInputBindingIdentity,
                            onChanged: (value) => unawaited(_setType(value)),
                          ),
                        ),
                        if (_control.deviceId != null)
                          Text(
                            'Device: ${_control.deviceId}',
                            style: const TextStyle(
                              color: CelestialColors.textSecondary,
                              fontSize: 11,
                            ),
                          ),
                        if (_error != null) ...[
                          const SizedBox(height: 8),
                          Text(
                            _error!,
                            style: const TextStyle(
                              color: _dangerAccent,
                              fontSize: 12,
                              fontWeight: FontWeight.w600,
                            ),
                          ),
                        ],
                      ],
                    ),
                  ),
                  const SizedBox(height: 12),
                  _ExpertPanel(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        Row(
                          children: [
                            const Text(
                              'Moment buttons',
                              style: TextStyle(
                                color: CelestialColors.textPrimary,
                                fontSize: 15,
                                fontWeight: FontWeight.w800,
                              ),
                            ),
                            const Spacer(),
                            Text(
                              '${_control.momentAssignmentCount}',
                              style: const TextStyle(
                                color: CelestialColors.textSecondary,
                                fontSize: 12,
                                fontWeight: FontWeight.w700,
                              ),
                            ),
                          ],
                        ),
                        const SizedBox(height: 10),
                        if (_buttonEvents.isEmpty)
                          const Text(
                            'No button events are known for this switch type.',
                            style: TextStyle(
                              color: CelestialColors.textSecondary,
                              fontSize: 12,
                            ),
                          )
                        else
                          for (final event in _buttonEvents)
                            _SwitchMagicRow(
                              event: event,
                              saving: _savingKey == 'magic:$event',
                              value: _magicDropdownValue(event),
                              items: _magicItems(event),
                              itemLabel: _magicItemLabel,
                              defaultAction: _magicActionLabel(
                                _type?.defaultMapping[event],
                                widget.moments,
                              ),
                              enabled: _modernExpertCanEditInputBindings,
                              onChanged: (value) =>
                                  unawaited(_setMagicButton(event, value)),
                            ),
                      ],
                    ),
                  ),
                  const SizedBox(height: 12),
                  _ExpertPanel(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        Row(
                          children: [
                            const Text(
                              'Reach',
                              style: TextStyle(
                                color: CelestialColors.textPrimary,
                                fontSize: 15,
                                fontWeight: FontWeight.w800,
                              ),
                            ),
                            const Spacer(),
                            OutlinedButton.icon(
                              onPressed:
                                  _saving || !_modernExpertCanEditInputBindings
                                      ? null
                                      : _addScope,
                              icon: const Icon(Icons.add_rounded, size: 17),
                              label: const Text('Scope'),
                              style: OutlinedButton.styleFrom(
                                foregroundColor: _expertAccent,
                                side: const BorderSide(
                                  color: CelestialColors.orbitRing,
                                ),
                                shape: RoundedRectangleBorder(
                                  borderRadius: BorderRadius.circular(8),
                                ),
                              ),
                            ),
                          ],
                        ),
                        const SizedBox(height: 10),
                        if (_control.scopes.isEmpty)
                          const Text(
                            'No reach scopes configured.',
                            style: TextStyle(
                              color: CelestialColors.textSecondary,
                              fontSize: 12,
                            ),
                          )
                        else
                          for (int i = 0; i < _control.scopes.length; i++)
                            _SwitchScopeCard(
                              index: i,
                              scope: _control.scopes[i],
                              areas: _areas,
                              sections: _sections,
                              areaName: _areaName,
                              sectionName: _sectionName,
                              saving: _savingKey == 'scope:$i',
                              enabled: _modernExpertCanEditInputBindings,
                              selectedAreaId: _areaPickerByScope[i],
                              onSelectedAreaChanged: (value) => setState(
                                () => _areaPickerByScope[i] = value,
                              ),
                              selectedSectionId: _sectionPickerByScope[i],
                              onSelectedSectionChanged: (value) => setState(
                                () => _sectionPickerByScope[i] = value,
                              ),
                              onAddArea: (areaId) =>
                                  unawaited(_addAreaToScope(i, areaId)),
                              onRemoveArea: (areaId) =>
                                  unawaited(_removeAreaFromScope(i, areaId)),
                              onAddSection: (sectionId) =>
                                  unawaited(_addSectionToScope(i, sectionId)),
                              onRemoveSection: (sectionId) => unawaited(
                                _removeSectionFromScope(i, sectionId),
                              ),
                              onSetFeedbackArea: (areaId) => unawaited(
                                _updateScope(
                                  i,
                                  _scopeWithFeedbackArea(
                                    _control.scopes[i],
                                    areaId,
                                  ),
                                ),
                              ),
                              canMoveUp: i > 0,
                              canMoveDown: i < _control.scopes.length - 1,
                              onMoveUp: () => unawaited(_moveScope(i, -1)),
                              onMoveDown: () => unawaited(_moveScope(i, 1)),
                              canRemoveScope: _control.scopes.length > 1,
                              onRemoveScope: () => unawaited(_removeScope(i)),
                            ),
                      ],
                    ),
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _ExpertZoneDefineScreen extends StatefulWidget {
  final _ExpertZone zone;
  final List<_ExpertZone> allZones;
  final _CircadianExpertClient? client;
  final bool showBackButton;
  final ValueChanged<_ExpertZone> onZoneChanged;

  const _ExpertZoneDefineScreen({
    required this.zone,
    required this.allZones,
    required this.client,
    this.showBackButton = true,
    required this.onZoneChanged,
  });

  @override
  State<_ExpertZoneDefineScreen> createState() =>
      _ExpertZoneDefineScreenState();
}

class _ExpertZoneDefineScreenState extends State<_ExpertZoneDefineScreen> {
  late _RhythmDefinition _draft = widget.zone.definition;
  late _RhythmDefinition _savedDefinition = widget.zone.definition;
  Map<String, _ExpertRhythmPreset> _rhythmPresets = const {};
  List<String> _presetOptions = _defaultRhythmPresetOptions;
  _ExpertSunTimes? _sunTimes;
  _ExpertCurveData? _serverCurve;
  _ExpertStepSequences? _stepSequences;
  Timer? _curveFetchDebounce;
  Timer? _stepFetchDebounce;
  bool _sunTimesLoading = false;
  bool _curveLoading = false;
  bool _stepsLoading = false;
  String? _sunTimesError;
  String? _curveError;
  String? _stepsError;
  bool _dirty = false;
  double _selectedHour = 13;
  String _openSection = 'sleep';

  @override
  void initState() {
    super.initState();
    unawaited(_loadSavedExpertDefinition());
    unawaited(_loadRhythmPresets());
    unawaited(_loadSunTimes());
    unawaited(_loadServerCurve());
    unawaited(_loadStepSequences());
  }

  @override
  void dispose() {
    _curveFetchDebounce?.cancel();
    _stepFetchDebounce?.cancel();
    super.dispose();
  }

  Future<void> _loadSavedExpertDefinition() async {
    final client = widget.client;
    if (client == null) return;
    try {
      final definition =
          await client.fetchExpertRhythmDefinition(widget.zone.id);
      if (!mounted) return;
      setState(() {
        _savedDefinition = definition;
        if (!_dirty) {
          _draft = definition;
          _dirty = false;
        }
      });
      unawaited(_loadServerCurve(silent: true));
      unawaited(_loadStepSequences(silent: true));
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: expert profile load failed: $error');
      debugPrint('$stackTrace');
    }
  }

  Future<void> _loadRhythmPresets() async {
    final client = widget.client;
    if (client == null) return;
    try {
      final load = await client.fetchRhythmPresets();
      if (!mounted) return;
      setState(() {
        _rhythmPresets = load.presets;
        _presetOptions = [
          for (final name in load.names)
            if (name != 'custom' || name == _draft.sleepPattern) name,
          if (!load.names.contains(_draft.sleepPattern)) _draft.sleepPattern,
        ];
      });
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: rhythm presets load failed: $error');
      debugPrint('$stackTrace');
    }
  }

  Future<void> _loadSunTimes() async {
    final client = widget.client;
    if (client == null) return;
    setState(() {
      _sunTimesLoading = true;
      _sunTimesError = null;
    });
    try {
      final sunTimes = await client.fetchSunTimes();
      if (!mounted) return;
      setState(() {
        _sunTimes = sunTimes;
        _sunTimesLoading = false;
      });
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: sun times load failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() {
        _sunTimesLoading = false;
        _sunTimesError = 'Solar timing unavailable: $error';
      });
    }
  }

  void _change(_RhythmDefinition Function(_RhythmDefinition) update) {
    setState(() {
      _draft = update(_draft);
      _dirty = _draft != _savedDefinition;
    });
    _scheduleCurveLoad();
    _scheduleStepLoad();
  }

  void _scheduleCurveLoad() {
    if (widget.client == null) return;
    _curveFetchDebounce?.cancel();
    _curveFetchDebounce = Timer(const Duration(milliseconds: 450), () {
      unawaited(_loadServerCurve(silent: true));
    });
  }

  Future<void> _loadServerCurve({bool silent = false}) async {
    final client = widget.client;
    if (client == null) return;
    if (!silent) {
      setState(() {
        _curveLoading = true;
        _curveError = null;
      });
    }
    try {
      final curve = await client.fetchCurveData(_draft);
      if (!mounted) return;
      setState(() {
        _serverCurve = curve;
        _curveLoading = false;
        _curveError = null;
      });
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: curve load failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() {
        _curveLoading = false;
        _curveError = 'Server curve unavailable: $error';
      });
    }
  }

  void _scheduleStepLoad() {
    if (widget.client == null) return;
    _stepFetchDebounce?.cancel();
    _stepFetchDebounce = Timer(const Duration(milliseconds: 350), () {
      unawaited(_loadStepSequences(silent: true));
    });
  }

  Future<void> _loadStepSequences({bool silent = false}) async {
    final client = widget.client;
    if (client == null) return;
    final requestDefinition = _draft;
    final requestHour = _selectedHour;
    if (!silent) {
      setState(() {
        _stepsLoading = true;
        _stepsError = null;
      });
    }
    try {
      final sequences = await client.fetchStepSequences(
        requestDefinition,
        hour: requestHour,
        maxSteps: 8,
      );
      if (!mounted ||
          _draft != requestDefinition ||
          (_selectedHour - requestHour).abs() > 0.02) {
        return;
      }
      setState(() {
        _stepSequences = sequences;
        _stepsLoading = false;
        _stepsError = null;
      });
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: step sequence load failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() {
        _stepsLoading = false;
        _stepsError = 'Step preview unavailable: $error';
      });
    }
  }

  void _applyPattern(String pattern) {
    final preset = _rhythmPresets[pattern];
    _change(
      (definition) {
        if (preset == null) {
          return definition.copyWith(
            sleepPattern: pattern,
            wakeHour: _patternWake(pattern),
            bedHour: _patternBed(pattern),
          );
        }
        return definition.copyWith(
          sleepPattern: pattern,
          wakeHour: preset.wakeHour,
          bedHour: preset.bedHour,
          phaseBalance: _phaseBalanceFromAscend(
            preset.ascendStart,
            preset.wakeHour,
          ),
        );
      },
    );
  }

  void _toggleSection(String id) {
    HapticFeedback.selectionClick();
    setState(() => _openSection = _openSection == id ? '' : id);
  }

  Future<void> _save() async {
    HapticFeedback.lightImpact();
    final client = widget.client;
    try {
      if (client != null) {
        await client.updateZoneSettings(widget.zone.name, _draft);
      }
      final updated = widget.zone.copyWith(definition: _draft);
      widget.onZoneChanged(updated);
      setState(() {
        _savedDefinition = _draft;
        _dirty = false;
      });
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          content: Text(
            client == null
                ? 'Expert rhythm saved locally'
                : 'Expert rhythm saved to RhythmOS',
          ),
        ),
      );
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: save failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('Expert rhythm save failed: $error')),
      );
    }
  }

  void _reset() {
    HapticFeedback.selectionClick();
    setState(() {
      _draft = _savedDefinition;
      _dirty = false;
    });
    _scheduleCurveLoad();
    _scheduleStepLoad();
  }

  void _openLive() {
    final nextZone = widget.zone.copyWith(definition: _draft);
    Navigator.of(context).pushReplacement(
      MaterialPageRoute(
        builder: (_) => _ExpertZoneLiveScreen(
          zone: nextZone,
          allZones: _zonesWithCurrent(widget.allZones, nextZone),
          client: widget.client,
          onZoneChanged: widget.onZoneChanged,
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final reading = _serverCurve?.readingAt(_selectedHour) ??
        _CurveReading.from(_draft, _selectedHour);
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            _ExpertHeader(
              title: 'Define rhythm',
              leading: widget.showBackButton
                  ? _BackButton(onPressed: () => Navigator.of(context).pop())
                  : const _ExpertModeBadge(),
              trailing: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  IconButton(
                    onPressed: _dirty ? _reset : null,
                    tooltip: 'Revert',
                    icon: const Icon(Icons.undo_rounded),
                  ),
                  IconButton(
                    onPressed: _dirty ? _save : null,
                    tooltip: 'Save',
                    icon: const Icon(Icons.save_rounded),
                  ),
                ],
              ),
            ),
            Expanded(
              child: ListView(
                padding: const EdgeInsets.fromLTRB(16, 0, 16, 28),
                children: [
                  _ZoneIdentityRow(
                    name: widget.zone.name,
                    pattern: _draft.sleepPattern,
                    patternOptions: _presetOptions,
                    dirty: _dirty,
                    onPatternChanged: _applyPattern,
                    onLive: _openLive,
                  ),
                  const SizedBox(height: 8),
                  _CurveCard(
                    title: '${reading.brightness}%  /  ${reading.kelvin} K',
                    subtitle: _curveLoading
                        ? 'loading server curve'
                        : _curveError == null
                            ? _formatHour(_selectedHour)
                            : 'local curve',
                    definition: _draft,
                    serverCurve: _serverCurve,
                    selectedHour: _selectedHour,
                    accent: _expertAccent,
                    onHourChanged: (hour) {
                      setState(() {
                        _selectedHour = hour;
                      });
                      _scheduleStepLoad();
                    },
                  ),
                  if (_curveError != null) ...[
                    const SizedBox(height: 7),
                    Text(
                      _curveError!,
                      style: const TextStyle(
                        color: _dangerAccent,
                        fontSize: 12,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                  ],
                  const SizedBox(height: 12),
                  _buildStepSequencePanel(),
                  const SizedBox(height: 12),
                  _buildSolarTimingPanel(),
                  const SizedBox(height: 12),
                  _TuneSectionCard(
                    id: 'sleep',
                    title: 'Sleep',
                    value: '${_formatHour(_draft.wakeHour)} wake, '
                        '${_formatHour(_draft.bedHour)} bed',
                    expanded: _openSection == 'sleep',
                    onToggle: () => _toggleSection('sleep'),
                    child: Column(
                      children: [
                        _OptionRow(
                          label: 'Pattern',
                          value: _draft.sleepPattern,
                          options: _presetOptions,
                          itemLabel: _patternLabel,
                          onChanged: _applyPattern,
                        ),
                        _ExpertSliderRow(
                          label: 'Wake',
                          value: _draft.wakeHour,
                          min: 3,
                          max: 11,
                          divisions: 32,
                          display: _formatHour(_draft.wakeHour),
                          onChanged: (value) => _change(
                            (definition) =>
                                definition.copyWith(wakeHour: value),
                          ),
                        ),
                        _ExpertSliderRow(
                          label: 'Bed',
                          value: _draft.bedHour < 12
                              ? _draft.bedHour + 24
                              : _draft.bedHour,
                          min: 18,
                          max: 27,
                          divisions: 36,
                          display: _formatHour(_draft.bedHour),
                          onChanged: (value) => _change(
                            (definition) => definition.copyWith(
                              bedHour: value >= 24 ? value - 24 : value,
                            ),
                          ),
                        ),
                        _ExpertSliderRow(
                          label: 'Sleep brightness',
                          value: _draft.sleepBrightness.toDouble(),
                          min: 0,
                          max: 35,
                          divisions: 35,
                          display: '${_draft.sleepBrightness}%',
                          onChanged: (value) => _change(
                            (definition) => definition.copyWith(
                              sleepBrightness: value.round(),
                            ),
                          ),
                        ),
                      ],
                    ),
                  ),
                  _TuneSectionCard(
                    id: 'brightness',
                    title: 'Brightness',
                    value: '${_draft.minBrightness}-${_draft.maxBrightness}%',
                    expanded: _openSection == 'brightness',
                    onToggle: () => _toggleSection('brightness'),
                    child: Column(
                      children: [
                        _ExpertSliderRow(
                          label: 'Minimum',
                          value: _draft.minBrightness.toDouble(),
                          min: 0,
                          max: 80,
                          divisions: 80,
                          display: '${_draft.minBrightness}%',
                          onChanged: (value) => _change(
                            (definition) => definition.copyWith(
                              minBrightness: math.min(
                                value.round(),
                                definition.maxBrightness - 1,
                              ),
                            ),
                          ),
                        ),
                        _ExpertSliderRow(
                          label: 'Peak',
                          value: _draft.maxBrightness.toDouble(),
                          min: 20,
                          max: 100,
                          divisions: 80,
                          display: '${_draft.maxBrightness}%',
                          onChanged: (value) => _change(
                            (definition) => definition.copyWith(
                              maxBrightness: math.max(
                                value.round(),
                                definition.minBrightness + 1,
                              ),
                            ),
                          ),
                        ),
                        _ExpertSliderRow(
                          label: 'Midday balance',
                          value: _draft.phaseBalance,
                          min: -1,
                          max: 1,
                          divisions: 40,
                          display: _signedPercent(_draft.phaseBalance),
                          onChanged: (value) => _change(
                            (definition) =>
                                definition.copyWith(phaseBalance: value),
                          ),
                        ),
                      ],
                    ),
                  ),
                  _TuneSectionCard(
                    id: 'color',
                    title: 'Color temperature',
                    value: '${_draft.minKelvin}-${_draft.maxKelvin} K',
                    expanded: _openSection == 'color',
                    onToggle: () => _toggleSection('color'),
                    child: Column(
                      children: [
                        _ExpertSliderRow(
                          label: 'Warm floor',
                          value: _draft.minKelvin.toDouble(),
                          min: 1800,
                          max: 4200,
                          divisions: 48,
                          display: '${_draft.minKelvin} K',
                          onChanged: (value) => _change(
                            (definition) => definition.copyWith(
                              minKelvin: math.min(
                                value.round(),
                                definition.maxKelvin - 100,
                              ),
                            ),
                          ),
                        ),
                        _ExpertSliderRow(
                          label: 'Cool peak',
                          value: _draft.maxKelvin.toDouble(),
                          min: 3000,
                          max: 6500,
                          divisions: 70,
                          display: '${_draft.maxKelvin} K',
                          onChanged: (value) => _change(
                            (definition) => definition.copyWith(
                              maxKelvin: math.max(
                                value.round(),
                                definition.minKelvin + 100,
                              ),
                            ),
                          ),
                        ),
                      ],
                    ),
                  ),
                  _TuneSectionCard(
                    id: 'sun',
                    title: 'Sun dimming',
                    value: '${(_draft.daylightDimming * 100).round()}%',
                    expanded: _openSection == 'sun',
                    onToggle: () => _toggleSection('sun'),
                    child: Column(
                      children: [
                        _ExpertSliderRow(
                          label: 'Daylight dimming',
                          value: _draft.daylightDimming,
                          min: 0,
                          max: 1,
                          divisions: 20,
                          display: '${(_draft.daylightDimming * 100).round()}%',
                          onChanged: (value) => _change(
                            (definition) =>
                                definition.copyWith(daylightDimming: value),
                          ),
                        ),
                        _ExpertSliderRow(
                          label: 'Natural exposure',
                          value: _draft.naturalExposure,
                          min: 0,
                          max: 1,
                          divisions: 20,
                          display: '${(_draft.naturalExposure * 100).round()}%',
                          onChanged: (value) => _change(
                            (definition) =>
                                definition.copyWith(naturalExposure: value),
                          ),
                        ),
                        _ExpertSliderRow(
                          label: 'Transition',
                          value: _draft.transitionMinutes,
                          min: 5,
                          max: 90,
                          divisions: 17,
                          display: '${_draft.transitionMinutes.round()} min',
                          onChanged: (value) => _change(
                            (definition) => definition.copyWith(
                              transitionMinutes: value,
                            ),
                          ),
                        ),
                      ],
                    ),
                  ),
                  const SizedBox(height: 8),
                  Row(
                    children: [
                      Expanded(
                        child: OutlinedButton.icon(
                          onPressed: _dirty ? _reset : null,
                          icon: const Icon(Icons.undo_rounded),
                          label: const Text('Revert'),
                          style: OutlinedButton.styleFrom(
                            foregroundColor: _changedAccent,
                            side: const BorderSide(color: _changedAccent),
                            padding: const EdgeInsets.symmetric(vertical: 13),
                          ),
                        ),
                      ),
                      const SizedBox(width: 12),
                      Expanded(
                        child: FilledButton.icon(
                          onPressed: _dirty ? _save : null,
                          icon: const Icon(Icons.save_rounded),
                          label: const Text('Save'),
                          style: FilledButton.styleFrom(
                            backgroundColor: _expertAccent,
                            foregroundColor: const Color(0xFF1A120C),
                            padding: const EdgeInsets.symmetric(vertical: 13),
                          ),
                        ),
                      ),
                    ],
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildStepSequencePanel() {
    final sequences = _stepSequences;
    return _ExpertPanel(
      padding: const EdgeInsets.fromLTRB(12, 11, 8, 12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(
                  'Switch steps at ${_formatHour(_selectedHour)}',
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 15,
                    fontWeight: FontWeight.w800,
                  ),
                ),
              ),
              if (_stepsLoading)
                const SizedBox(
                  width: 16,
                  height: 16,
                  child: CircularProgressIndicator(strokeWidth: 2),
                ),
              IconButton(
                onPressed: _stepsLoading ? null : _loadStepSequences,
                tooltip: 'Refresh switch steps',
                icon: const Icon(Icons.refresh_rounded),
                iconSize: 18,
                visualDensity: VisualDensity.compact,
                color: CelestialColors.textSecondary,
              ),
            ],
          ),
          if (_stepsError != null) ...[
            const SizedBox(height: 6),
            Text(
              _stepsError!,
              style: const TextStyle(
                color: _dangerAccent,
                fontSize: 12,
                fontWeight: FontWeight.w600,
              ),
            ),
          ],
          const SizedBox(height: 8),
          LayoutBuilder(
            builder: (context, constraints) {
              final up = _buildStepSequenceColumn(
                label: 'Brighten',
                icon: Icons.add_rounded,
                points: sequences?.stepUp ?? const [],
              );
              final down = _buildStepSequenceColumn(
                label: 'Dim',
                icon: Icons.remove_rounded,
                points: sequences?.stepDown ?? const [],
              );
              if (constraints.maxWidth < 380) {
                return Column(
                  children: [
                    up,
                    const SizedBox(height: 8),
                    down,
                  ],
                );
              }
              return Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Expanded(child: up),
                  const SizedBox(width: 8),
                  Expanded(child: down),
                ],
              );
            },
          ),
        ],
      ),
    );
  }

  Widget _buildStepSequenceColumn({
    required String label,
    required IconData icon,
    required List<_ExpertStepPoint> points,
  }) {
    final visiblePoints = points.take(5).toList(growable: false);
    return Container(
      padding: const EdgeInsets.fromLTRB(10, 9, 10, 10),
      decoration: BoxDecoration(
        color: _panel2Color.withValues(alpha: 0.62),
        borderRadius: BorderRadius.circular(7),
        border: Border.all(color: CelestialColors.orbitRing),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Icon(icon, color: _expertAccent, size: 15),
              const SizedBox(width: 6),
              Text(
                label,
                style: const TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 12,
                  fontWeight: FontWeight.w800,
                ),
              ),
            ],
          ),
          const SizedBox(height: 8),
          if (visiblePoints.isEmpty)
            Text(
              widget.client == null ? 'server offline' : 'no sequence',
              style: const TextStyle(
                color: CelestialColors.textSecondary,
                fontSize: 12,
                fontWeight: FontWeight.w600,
              ),
            )
          else
            for (int i = 0; i < visiblePoints.length; i++)
              Padding(
                padding: EdgeInsets.only(top: i == 0 ? 0 : 5),
                child: _buildStepPointRow(visiblePoints[i], current: i == 0),
              ),
        ],
      ),
    );
  }

  Widget _buildStepPointRow(_ExpertStepPoint point, {required bool current}) {
    return Row(
      children: [
        SizedBox(
          width: 44,
          child: Text(
            current ? 'now' : _formatHour(point.hour),
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(
              color: current ? _expertAccent : CelestialColors.textSecondary,
              fontSize: 11,
              fontWeight: FontWeight.w800,
              fontFeatures: const [FontFeature.tabularFigures()],
            ),
          ),
        ),
        Expanded(
          child: Text(
            '${point.brightness}% / ${point.kelvin} K',
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            textAlign: TextAlign.right,
            style: const TextStyle(
              color: CelestialColors.textPrimary,
              fontSize: 12,
              fontWeight: FontWeight.w700,
              fontFeatures: [FontFeature.tabularFigures()],
            ),
          ),
        ),
      ],
    );
  }

  Widget _buildSolarTimingPanel() {
    final sunTimes = _sunTimes;
    return _ExpertPanel(
      padding: const EdgeInsets.fromLTRB(12, 11, 8, 12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              const Expanded(
                child: Text(
                  'Solar timing',
                  style: TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 15,
                    fontWeight: FontWeight.w800,
                  ),
                ),
              ),
              if (_sunTimesLoading)
                const SizedBox(
                  width: 16,
                  height: 16,
                  child: CircularProgressIndicator(strokeWidth: 2),
                ),
              IconButton(
                onPressed: _sunTimesLoading ? null : _loadSunTimes,
                tooltip: 'Refresh solar timing',
                icon: const Icon(Icons.refresh_rounded),
                iconSize: 18,
                visualDensity: VisualDensity.compact,
                color: CelestialColors.textSecondary,
              ),
            ],
          ),
          if (_sunTimesError != null) ...[
            const SizedBox(height: 6),
            Text(
              _sunTimesError!,
              style: const TextStyle(
                color: _dangerAccent,
                fontSize: 12,
                fontWeight: FontWeight.w600,
              ),
            ),
          ],
          const SizedBox(height: 8),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [
              _AreaMetricChip(
                icon: Icons.wb_twilight_rounded,
                label: 'sunrise',
                value: sunTimes == null ? '-' : _formatHour(sunTimes.sunrise),
              ),
              _AreaMetricChip(
                icon: Icons.wb_sunny_rounded,
                label: 'noon',
                value: sunTimes == null ? '-' : _formatHour(sunTimes.noon),
              ),
              _AreaMetricChip(
                icon: Icons.nightlight_round,
                label: 'sunset',
                value: sunTimes == null ? '-' : _formatHour(sunTimes.sunset),
              ),
              _AreaMetricChip(
                icon: Icons.dark_mode_rounded,
                label: 'midnight',
                value: sunTimes == null ? '-' : _formatHour(sunTimes.midnight),
              ),
            ],
          ),
        ],
      ),
    );
  }
}

class _ExpertZoneLiveScreen extends StatefulWidget {
  final _ExpertZone zone;
  final List<_ExpertZone> allZones;
  final _CircadianExpertClient? client;
  final ValueChanged<_ExpertZone> onZoneChanged;

  const _ExpertZoneLiveScreen({
    required this.zone,
    required this.allZones,
    required this.client,
    required this.onZoneChanged,
  });

  @override
  State<_ExpertZoneLiveScreen> createState() => _ExpertZoneLiveScreenState();
}

class _ExpertZoneLiveScreenState extends State<_ExpertZoneLiveScreen> {
  static const _livePreviewHeartbeat = Duration(seconds: 30);

  late _ExpertZone _zone = widget.zone;
  double _selectedHour = _decimalNow(TimeOfDay.now());
  bool _powerOn = true;
  bool _frozen = false;
  bool _boosted = false;
  bool _circadianOn = true;
  bool _livePreviewActive = false;
  bool _livePreviewBusy = false;
  String? _zoneActionBusy;
  _ExpertAreaStatus? _serverStatus;
  _ExpertZoneSchedule? _zoneSchedule;
  List<_ExpertHistoryEntry> _zoneHistoryEntries = const [];
  String? _zoneHistoryHint;
  bool _serverLoading = false;
  bool _scheduleSaving = false;
  String? _serverError;
  String? _scheduleError;
  Timer? _livePreviewHeartbeatTimer;
  Timer? _livePreviewApplyTimer;

  List<_ExpertZone> get _allZonesForDetail {
    return _zonesWithCurrent(widget.allZones, _zone);
  }

  @override
  void initState() {
    super.initState();
    unawaited(_loadZoneServerState());
  }

  @override
  void dispose() {
    _livePreviewHeartbeatTimer?.cancel();
    _livePreviewApplyTimer?.cancel();
    final client = widget.client;
    if (_livePreviewActive && client != null) {
      unawaited(
        client.setCircadianModeForAreas(_areaIds, enabled: true),
      );
    }
    super.dispose();
  }

  Future<void> _loadZoneServerState({bool silent = false}) async {
    final client = widget.client;
    if (client == null) {
      if (!mounted) return;
      setState(() {
        _serverLoading = false;
        _serverError = 'Connect to RhythmOS to load server state.';
      });
      return;
    }

    if (!silent) {
      setState(() {
        _serverLoading = true;
        _serverError = null;
      });
    }

    try {
      final results = await Future.wait<Object?>([
        client.fetchZoneAdjust(_zone.name),
        client.fetchZoneHistory(_zone.name, limit: 12).catchError((
          Object error,
          StackTrace stackTrace,
        ) {
          debugPrint('CircadianExpert: zone history load failed: $error');
          debugPrint('$stackTrace');
          return const _ExpertAreaHistory(
            entries: [],
            hint: 'Zone history unavailable.',
          );
        }),
        client.fetchZoneSchedule(_zone.name).catchError((
          Object error,
          StackTrace stackTrace,
        ) {
          debugPrint('CircadianExpert: zone schedule load failed: $error');
          debugPrint('$stackTrace');
          return _ExpertZoneSchedule.unavailable(
            'Zone schedule unavailable: $error',
          );
        }),
      ]);
      if (!mounted) return;
      final status = results[0] as _ExpertAreaStatus;
      final history = results[1] as _ExpertAreaHistory;
      final schedule = results[2] as _ExpertZoneSchedule;
      setState(() {
        _serverStatus = status;
        _zoneSchedule = schedule;
        _frozen = status.frozen;
        _circadianOn = status.isCircadian;
        _zoneHistoryEntries = history.entries;
        _zoneHistoryHint = history.hint;
        _serverLoading = false;
        _serverError = null;
        _scheduleError = schedule.error;
      });
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: zone server load failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() {
        _serverLoading = false;
        _serverError = 'Zone server load failed: $error';
      });
    }
  }

  void _togglePower() {
    HapticFeedback.selectionClick();
    setState(() => _powerOn = !_powerOn);
    _sendPreviewIfActive();
  }

  void _toggleFrozen() {
    unawaited(
      _runZoneAction(
        'freeze_toggle',
        optimisticFrozen: !_frozen,
      ),
    );
  }

  void _toggleBoost() {
    HapticFeedback.selectionClick();
    setState(() => _boosted = !_boosted);
    _sendPreviewIfActive();
  }

  void _toggleCircadian() {
    HapticFeedback.selectionClick();
    if (_livePreviewActive) {
      unawaited(_endLivePreview());
      return;
    }
    final next = !_circadianOn;
    setState(() => _circadianOn = next);
    unawaited(_persistCircadianMode(next));
  }

  Future<void> _persistCircadianMode(bool enabled) async {
    final client = widget.client;
    if (client == null) return;
    try {
      await client.setCircadianModeForAreas(
        _zone.areas.map((area) => area.id),
        enabled: enabled,
      );
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: circadian toggle failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('Circadian mode update failed: $error')),
      );
    }
  }

  Future<void> _runZoneAction(
    String action, {
    Map<String, Object?> extra = const {},
    bool? optimisticFrozen,
    bool confirmCascade = false,
  }) async {
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before adjusting this zone.');
      return;
    }

    if (confirmCascade) {
      final confirmed = await showDialog<bool>(
        context: context,
        builder: (context) => AlertDialog(
          backgroundColor: CelestialColors.backgroundCard,
          title: const Text(
            'Reset zone and areas',
            style: TextStyle(color: CelestialColors.textPrimary),
          ),
          content: Text(
            'Reset "${_zone.name}" and every area in it?',
            style: const TextStyle(color: CelestialColors.textSecondary),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.of(context).pop(false),
              child: const Text('Cancel'),
            ),
            TextButton(
              onPressed: () => Navigator.of(context).pop(true),
              child: const Text(
                'Reset',
                style: TextStyle(color: _dangerAccent),
              ),
            ),
          ],
        ),
      );
      if (confirmed != true) return;
    }

    HapticFeedback.selectionClick();
    final previousFrozen = _frozen;
    final nextFrozen =
        action == 'freeze_toggle' ? (optimisticFrozen ?? !_frozen) : null;
    setState(() {
      _zoneActionBusy = action;
      if (nextFrozen != null) _frozen = nextFrozen;
    });

    try {
      await client.runZoneAction(
        _zone.name,
        action,
        {
          ...extra,
          if (nextFrozen != null) 'enabled': nextFrozen,
          if (nextFrozen != null) 'frozen_at_hour': _selectedHour,
        },
      );
      await _loadZoneServerState(silent: true);
      if (!mounted) return;
      if (action == 'glozone_reset_frozen' ||
          action == 'reset_zone' ||
          action == 'reset_zone_cascade') {
        setState(() => _frozen = false);
      } else if (nextFrozen != null) {
        setState(() => _frozen = nextFrozen);
      }
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: zone action failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() => _frozen = previousFrozen);
      _showSnack('Zone action failed: $error');
    } finally {
      if (mounted) setState(() => _zoneActionBusy = null);
    }
  }

  Future<void> _saveZoneScheduleOverride(
    String mode, {
    double? customWake,
    double? customBed,
  }) async {
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before changing this schedule.');
      return;
    }

    HapticFeedback.selectionClick();
    setState(() {
      _scheduleSaving = true;
      _scheduleError = null;
    });
    try {
      await client.setZoneScheduleOverride(
        _zone.name,
        mode: mode,
        customWake: customWake,
        customBed: customBed,
      );
      await _loadZoneServerState(silent: true);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: zone schedule save failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() => _scheduleError = 'Schedule save failed: $error');
      _showSnack('Schedule save failed: $error');
    } finally {
      if (mounted) setState(() => _scheduleSaving = false);
    }
  }

  Future<void> _clearZoneScheduleOverride() async {
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before changing this schedule.');
      return;
    }

    HapticFeedback.selectionClick();
    setState(() {
      _scheduleSaving = true;
      _scheduleError = null;
    });
    try {
      await client.clearZoneScheduleOverride(_zone.name);
      await _loadZoneServerState(silent: true);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: zone schedule clear failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() => _scheduleError = 'Schedule clear failed: $error');
      _showSnack('Schedule clear failed: $error');
    } finally {
      if (mounted) setState(() => _scheduleSaving = false);
    }
  }

  Future<void> _openCustomScheduleOverride() async {
    var wake = _zoneSchedule?.customWake ?? _zone.definition.wakeHour;
    var bed = _zoneSchedule?.customBed ?? _zone.definition.bedHour;
    final result = await showDialog<Map<String, double>>(
      context: context,
      builder: (dialogContext) => StatefulBuilder(
        builder: (context, setDialogState) {
          Future<void> pickTime(String field, double current) async {
            final picked = await showTimePicker(
              context: context,
              initialTime: _timeOfDayFromHour(current),
              builder: (context, child) {
                return Theme(
                  data: Theme.of(context).copyWith(
                    colorScheme: const ColorScheme.dark(
                      primary: _expertAccent,
                      surface: _panelColor,
                    ),
                  ),
                  child: child!,
                );
              },
            );
            if (picked == null) return;
            setDialogState(() {
              final value = _roundQuarter(picked.hour + picked.minute / 60);
              if (field == 'wake') {
                wake = value;
              } else {
                bed = value;
              }
            });
          }

          return AlertDialog(
            backgroundColor: CelestialColors.backgroundCard,
            title: const Text(
              'Custom schedule',
              style: TextStyle(color: CelestialColors.textPrimary),
            ),
            content: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                _TimeSettingRow(
                  label: 'Wake',
                  value: wake,
                  onPick: () => unawaited(pickTime('wake', wake)),
                  onClear: null,
                ),
                const SizedBox(height: 8),
                _TimeSettingRow(
                  label: 'Bed',
                  value: bed,
                  onPick: () => unawaited(pickTime('bed', bed)),
                  onClear: null,
                ),
              ],
            ),
            actions: [
              TextButton(
                onPressed: () => Navigator.of(dialogContext).pop(),
                child: const Text('Cancel'),
              ),
              TextButton(
                onPressed: () => Navigator.of(dialogContext).pop({
                  'wake': wake,
                  'bed': bed,
                }),
                child: const Text('Save'),
              ),
            ],
          );
        },
      ),
    );
    if (result == null) return;
    await _saveZoneScheduleOverride(
      'custom',
      customWake: result['wake'],
      customBed: result['bed'],
    );
  }

  Future<void> _toggleLivePreview() async {
    if (_livePreviewActive) {
      await _endLivePreview();
    } else {
      await _startLivePreview();
    }
  }

  Future<void> _startLivePreview() async {
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before starting live preview.');
      return;
    }
    final areaIds = _areaIds;
    if (areaIds.isEmpty) {
      _showSnack('Assign areas to this rhythm zone before live preview.');
      return;
    }

    setState(() => _livePreviewBusy = true);
    try {
      await client.setCircadianModeForAreas(areaIds, enabled: false);
      await client.sendLiveDesignHeartbeat(areaIds);
      if (!mounted) return;
      setState(() {
        _livePreviewActive = true;
        _circadianOn = false;
      });
      _startLivePreviewHeartbeat(areaIds);
      await Future<void>.delayed(const Duration(milliseconds: 2200));
      await _applyLivePreview();
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: live preview start failed: $error');
      debugPrint('$stackTrace');
      _showSnack('Live preview failed: $error');
    } finally {
      if (mounted) setState(() => _livePreviewBusy = false);
    }
  }

  Future<void> _endLivePreview({
    bool restoreCircadian = true,
    bool showErrors = true,
  }) async {
    final client = widget.client;
    final areaIds = _areaIds;
    _livePreviewHeartbeatTimer?.cancel();
    _livePreviewHeartbeatTimer = null;
    _livePreviewApplyTimer?.cancel();
    _livePreviewApplyTimer = null;
    if (mounted) {
      setState(() {
        _livePreviewActive = false;
        _livePreviewBusy = true;
      });
    }

    try {
      if (client != null && restoreCircadian && areaIds.isNotEmpty) {
        await client.setCircadianModeForAreas(areaIds, enabled: true);
        await client.endLiveDesign(areaIds);
      }
      if (mounted) {
        setState(() => _circadianOn = true);
      }
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: live preview stop failed: $error');
      debugPrint('$stackTrace');
      if (showErrors) _showSnack('Could not stop live preview cleanly: $error');
    } finally {
      if (mounted) setState(() => _livePreviewBusy = false);
    }
  }

  void _startLivePreviewHeartbeat(List<String> areaIds) {
    _livePreviewHeartbeatTimer?.cancel();
    _livePreviewHeartbeatTimer = Timer.periodic(_livePreviewHeartbeat, (_) {
      final client = widget.client;
      if (client == null || !_livePreviewActive) return;
      unawaited(client.sendLiveDesignHeartbeat(areaIds));
    });
  }

  void _sendPreviewIfActive() {
    if (!_livePreviewActive) return;
    _livePreviewApplyTimer?.cancel();
    _livePreviewApplyTimer = Timer(const Duration(milliseconds: 120), () {
      unawaited(_applyLivePreview());
    });
  }

  Future<void> _applyLivePreview() async {
    final client = widget.client;
    if (client == null || !_livePreviewActive) return;
    final reading = _CurveReading.from(_zone.definition, _selectedHour);
    final brightness =
        _powerOn ? math.min(100, reading.brightness + (_boosted ? 20 : 0)) : 1;
    try {
      await client.applyLightToAreas(
        _areaIds,
        brightness: brightness,
        kelvin: reading.kelvin,
        transitionSeconds: 0.2,
      );
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: live preview apply failed: $error');
      debugPrint('$stackTrace');
      if (mounted) _showSnack('Live preview update failed: $error');
    }
  }

  void _setSelectedHour(double hour) {
    setState(() {
      _selectedHour = hour;
    });
    _sendPreviewIfActive();
  }

  List<String> get _areaIds => _zone.areas.map((area) => area.id).toList();

  void _showSnack(String message) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(content: Text(message)),
    );
  }

  void _openDefine() {
    Navigator.of(context).pushReplacement(
      MaterialPageRoute(
        builder: (_) => _ExpertZoneDefineScreen(
          zone: _zone,
          allZones: _allZonesForDetail,
          client: widget.client,
          onZoneChanged: (zone) {
            widget.onZoneChanged(zone);
            _zone = zone;
          },
        ),
      ),
    );
  }

  void _openAreaDetail(_ExpertArea area, _CurveReading reading) {
    Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) => _ExpertAreaDetailScreen(
          zone: _zone,
          zones: _allZonesForDetail,
          area: area,
          client: widget.client,
          seedReading: reading,
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final reading = _CurveReading.from(_zone.definition, _selectedHour);
    final effectiveBrightness =
        _powerOn ? math.min(100, reading.brightness + (_boosted ? 20 : 0)) : 0;
    final serverStatus = _serverStatus;
    final displayBrightness = _livePreviewActive
        ? effectiveBrightness
        : serverStatus?.actualBrightness ?? effectiveBrightness;
    final displayKelvin = _livePreviewActive
        ? reading.kelvin
        : serverStatus?.kelvin ?? reading.kelvin;
    final tint = _cctToColor(displayKelvin);
    final headerInk = _readableOn(tint);

    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            AnimatedContainer(
              duration: const Duration(milliseconds: 180),
              decoration: BoxDecoration(
                color: Color.lerp(_panelColor, tint, _powerOn ? 0.28 : 0.05),
                border: const Border(
                  bottom: BorderSide(color: CelestialColors.orbitRing),
                ),
                borderRadius: const BorderRadius.only(
                  bottomLeft: Radius.circular(12),
                  bottomRight: Radius.circular(12),
                ),
              ),
              padding: const EdgeInsets.fromLTRB(12, 10, 12, 12),
              child: Column(
                children: [
                  Row(
                    crossAxisAlignment: CrossAxisAlignment.center,
                    children: [
                      _BackButton(
                        onPressed: () => Navigator.of(context).pop(),
                        ink: headerInk,
                        background: Colors.black.withValues(alpha: 0.18),
                      ),
                      const SizedBox(width: 10),
                      Expanded(
                        child: Text(
                          _zone.name,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                            color: headerInk,
                            fontSize: 22,
                            fontWeight: FontWeight.w700,
                          ),
                        ),
                      ),
                      Text(
                        _powerOn
                            ? '$displayBrightness% / $displayKelvin K'
                            : 'Off',
                        style: TextStyle(
                          color:
                              headerInk.withValues(alpha: _powerOn ? 1 : 0.6),
                          fontSize: 18,
                          fontWeight: FontWeight.w700,
                        ),
                      ),
                    ],
                  ),
                  Padding(
                    padding: const EdgeInsets.only(left: 50, top: 2),
                    child: Row(
                      children: [
                        Expanded(
                          child: Text(
                            'in rhythm zone',
                            style: TextStyle(
                              color: headerInk.withValues(alpha: 0.75),
                              fontSize: 12,
                              fontWeight: FontWeight.w600,
                            ),
                          ),
                        ),
                        TextButton(
                          onPressed: _openDefine,
                          style: TextButton.styleFrom(
                            foregroundColor: headerInk,
                            visualDensity: VisualDensity.compact,
                          ),
                          child: const Text('Define'),
                        ),
                      ],
                    ),
                  ),
                  const SizedBox(height: 4),
                  Row(
                    children: [
                      _LiveAction(
                        label: 'Power',
                        value: _powerOn ? 'on' : 'off',
                        active: _powerOn,
                        ink: headerInk,
                        onTap: _togglePower,
                      ),
                      _LiveAction(
                        label: 'Freeze',
                        value: _frozen ? 'held' : 'auto',
                        active: _frozen,
                        ink: headerInk,
                        onTap: _toggleFrozen,
                      ),
                      _LiveAction(
                        label: 'Boost',
                        value: _boosted ? '+20%' : 'normal',
                        active: _boosted,
                        ink: headerInk,
                        onTap: _toggleBoost,
                      ),
                      const Spacer(),
                      _LiveAction(
                        label: 'Circadian',
                        value: _circadianOn ? 'on' : 'off',
                        active: _circadianOn,
                        ink: headerInk,
                        onTap: _toggleCircadian,
                      ),
                    ],
                  ),
                ],
              ),
            ),
            Expanded(
              child: ListView(
                padding: const EdgeInsets.fromLTRB(16, 14, 16, 28),
                children: [
                  _CurveCard(
                    title: _formatHour(_selectedHour),
                    subtitle: _livePreviewActive
                        ? 'Broadcasting raw preview'
                        : serverStatus != null
                            ? 'Server target from RhythmOS'
                            : _circadianOn
                                ? 'Live target follows the saved rhythm'
                                : 'Manual light state',
                    definition: _zone.definition,
                    selectedHour: _selectedHour,
                    accent: _circadianOn ? _expertAccent : _changedAccent,
                    onHourChanged: _setSelectedHour,
                  ),
                  const SizedBox(height: 10),
                  _LivePreviewButton(
                    active: _livePreviewActive,
                    busy: _livePreviewBusy,
                    enabled: widget.client != null && _zone.areas.isNotEmpty,
                    onPressed: _toggleLivePreview,
                  ),
                  const SizedBox(height: 14),
                  _buildZoneAdjustPanel(),
                  const SizedBox(height: 14),
                  _buildZoneServerPanel(serverStatus),
                  const SizedBox(height: 14),
                  _buildZoneSchedulePanel(),
                  const SizedBox(height: 14),
                  _ExpertPanel(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        const Text(
                          'Areas',
                          style: TextStyle(
                            color: CelestialColors.textPrimary,
                            fontSize: 16,
                            fontWeight: FontWeight.w700,
                          ),
                        ),
                        const SizedBox(height: 10),
                        if (_zone.areas.isEmpty)
                          Text(
                            'No areas are assigned to this rhythm zone.',
                            style: TextStyle(
                              color: CelestialColors.textSecondary
                                  .withValues(alpha: 0.85),
                              fontSize: 13,
                            ),
                          )
                        else
                          for (final area in _zone.areas)
                            _AreaLiveRow(
                              area: area,
                              reading: reading,
                              powerOn: _powerOn,
                              boosted: _boosted,
                              onTap: () => _openAreaDetail(area, reading),
                            ),
                      ],
                    ),
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildZoneServerPanel(_ExpertAreaStatus? status) {
    final metrics = <Widget>[
      _AreaMetricChip(
        icon: Icons.light_mode_rounded,
        label: 'target',
        value:
            '${status?.actualBrightness ?? _CurveReading.from(_zone.definition, _selectedHour).brightness}%',
      ),
      _AreaMetricChip(
        icon: Icons.thermostat_rounded,
        label: 'color',
        value:
            '${status?.kelvin ?? _CurveReading.from(_zone.definition, _selectedHour).kelvin} K',
      ),
      _AreaMetricChip(
        icon: Icons.tune_rounded,
        label: 'range',
        value: status == null
            ? '${_zone.definition.minBrightness}-${_zone.definition.maxBrightness}%'
            : '${status.minBrightness}-${status.maxBrightness}%',
      ),
    ];

    final phase = status?.phase;
    if (phase != null) {
      metrics.add(
        _AreaMetricChip(
          icon: Icons.timeline_rounded,
          label: 'phase',
          value: phase,
        ),
      );
    }
    if (status?.frozen == true) {
      metrics.add(
        const _AreaMetricChip(
          icon: Icons.pause_circle_outline_rounded,
          label: 'freeze',
          value: 'held',
        ),
      );
    }
    if (status?.hasBrightnessOverride == true) {
      metrics.add(
        _AreaMetricChip(
          icon: Icons.exposure_rounded,
          label: 'bright',
          value: _signedValue(status!.brightnessOverride!),
        ),
      );
    }
    if (status?.hasColorOverride == true) {
      metrics.add(
        _AreaMetricChip(
          icon: Icons.thermostat_auto_rounded,
          label: 'color adj',
          value: _signedValue(status!.colorOverride!),
        ),
      );
    }
    final outdoor = status?.outdoorNormalized;
    if (outdoor != null) {
      metrics.add(
        _AreaMetricChip(
          icon: Icons.wb_sunny_outlined,
          label: 'outside',
          value: '${(outdoor * 100).round()}%',
        ),
      );
    }

    return _ExpertPanel(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              const Expanded(
                child: Text(
                  'Server State',
                  style: TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 16,
                    fontWeight: FontWeight.w700,
                  ),
                ),
              ),
              if (_serverLoading)
                const SizedBox(
                  width: 16,
                  height: 16,
                  child: CircularProgressIndicator(strokeWidth: 2),
                ),
              IconButton(
                onPressed: _serverLoading ? null : () => _loadZoneServerState(),
                tooltip: 'Refresh zone server state',
                icon: const Icon(Icons.refresh_rounded),
                iconSize: 18,
                visualDensity: VisualDensity.compact,
                color: CelestialColors.textSecondary,
              ),
            ],
          ),
          if (_serverError != null) ...[
            const SizedBox(height: 8),
            Text(
              _serverError!,
              style: const TextStyle(
                color: _dangerAccent,
                fontSize: 12,
                fontWeight: FontWeight.w600,
              ),
            ),
          ],
          const SizedBox(height: 10),
          Wrap(spacing: 8, runSpacing: 8, children: metrics),
          const SizedBox(height: 12),
          _HistoryPreview(
            entries: _zoneHistoryEntries,
            hint: _zoneHistoryHint,
            onRefresh: () => _loadZoneServerState(silent: true),
          ),
        ],
      ),
    );
  }

  Widget _buildZoneSchedulePanel() {
    final schedule = _zoneSchedule;
    final hasOverride = schedule?.hasOverride == true;
    final scheduleUnavailable = schedule?.error != null;
    final busy = _serverLoading || _scheduleSaving || scheduleUnavailable;
    final wakeLabel =
        schedule?.nextWakeLabel ?? _formatHour(_zone.definition.wakeHour);
    final bedLabel =
        schedule?.nextBedLabel ?? _formatHour(_zone.definition.bedHour);

    return _ExpertPanel(
      padding: const EdgeInsets.all(12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              const Expanded(
                child: Text(
                  'Schedule',
                  style: TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 16,
                    fontWeight: FontWeight.w800,
                  ),
                ),
              ),
              if (_scheduleSaving)
                const SizedBox(
                  width: 16,
                  height: 16,
                  child: CircularProgressIndicator(strokeWidth: 2),
                ),
            ],
          ),
          const SizedBox(height: 10),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [
              _AreaMetricChip(
                icon: Icons.wb_twilight_rounded,
                label: 'wake',
                value: wakeLabel,
              ),
              _AreaMetricChip(
                icon: Icons.nightlight_round,
                label: 'bed',
                value: bedLabel,
              ),
              _AreaMetricChip(
                icon: Icons.event_repeat_rounded,
                label: 'override',
                value: schedule?.overrideLabel ?? 'none',
              ),
            ],
          ),
          if (_scheduleError != null) ...[
            const SizedBox(height: 8),
            Text(
              _scheduleError!,
              style: const TextStyle(
                color: _dangerAccent,
                fontSize: 12,
                fontWeight: FontWeight.w600,
              ),
            ),
          ],
          const SizedBox(height: 10),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [
              _CompactTextButton(
                label: 'Main',
                onPressed:
                    busy ? null : () => _saveZoneScheduleOverride('main'),
              ),
              _CompactTextButton(
                label: 'Alt',
                onPressed: busy ? null : () => _saveZoneScheduleOverride('alt'),
              ),
              _CompactTextButton(
                label: 'Custom',
                onPressed: busy ? null : _openCustomScheduleOverride,
              ),
              _CompactTextButton(
                label: 'Off',
                color: _changedAccent,
                onPressed: busy ? null : () => _saveZoneScheduleOverride('off'),
              ),
              if (hasOverride)
                _CompactTextButton(
                  label: 'Clear',
                  color: _dangerAccent,
                  onPressed: busy ? null : _clearZoneScheduleOverride,
                ),
            ],
          ),
        ],
      ),
    );
  }

  Widget _buildZoneAdjustPanel() {
    final busy = _zoneActionBusy != null;
    return _ExpertPanel(
      padding: const EdgeInsets.all(12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              const Expanded(
                child: Text(
                  'Zone adjust',
                  style: TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 16,
                    fontWeight: FontWeight.w800,
                  ),
                ),
              ),
              if (busy)
                const SizedBox(
                  width: 16,
                  height: 16,
                  child: CircularProgressIndicator(strokeWidth: 2),
                ),
            ],
          ),
          const SizedBox(height: 10),
          Row(
            children: [
              Expanded(
                child: _ZoneSyncButton(
                  icon: Icons.remove_rounded,
                  label: 'Dim',
                  active: _zoneActionBusy == 'bright_down',
                  enabled: !busy,
                  onPressed: () => _runZoneAction('bright_down'),
                ),
              ),
              const SizedBox(width: 8),
              Expanded(
                child: _ZoneSyncButton(
                  icon: Icons.add_rounded,
                  label: 'Bright',
                  active: _zoneActionBusy == 'bright_up',
                  enabled: !busy,
                  onPressed: () => _runZoneAction('bright_up'),
                ),
              ),
              const SizedBox(width: 8),
              Expanded(
                child: _ZoneSyncButton(
                  icon: Icons.ac_unit_rounded,
                  label: 'Cooler',
                  active: _zoneActionBusy == 'color_up',
                  enabled: !busy,
                  onPressed: () => _runZoneAction('color_up'),
                ),
              ),
              const SizedBox(width: 8),
              Expanded(
                child: _ZoneSyncButton(
                  icon: Icons.local_fire_department_rounded,
                  label: 'Warmer',
                  active: _zoneActionBusy == 'color_down',
                  enabled: !busy,
                  onPressed: () => _runZoneAction('color_down'),
                ),
              ),
            ],
          ),
          const SizedBox(height: 8),
          Row(
            children: [
              Expanded(
                child: _ZoneSyncButton(
                  icon: Icons.keyboard_arrow_left_rounded,
                  label: 'Earlier',
                  active: _zoneActionBusy == 'step_down',
                  enabled: !busy,
                  onPressed: () => _runZoneAction('step_down'),
                ),
              ),
              const SizedBox(width: 8),
              Expanded(
                child: _ZoneSyncButton(
                  icon: Icons.keyboard_arrow_right_rounded,
                  label: 'Later',
                  active: _zoneActionBusy == 'step_up',
                  enabled: !busy,
                  onPressed: () => _runZoneAction('step_up'),
                ),
              ),
              const SizedBox(width: 8),
              Expanded(
                child: _ZoneSyncButton(
                  icon: Icons.pause_circle_outline_rounded,
                  label: _frozen ? 'Unfreeze' : 'Freeze',
                  active: _zoneActionBusy == 'freeze_toggle' || _frozen,
                  enabled: !busy,
                  highlighted: _frozen,
                  onPressed: _toggleFrozen,
                ),
              ),
              const SizedBox(width: 8),
              Expanded(
                child: _ZoneSyncButton(
                  icon: Icons.restart_alt_rounded,
                  label: 'Reset',
                  active: _zoneActionBusy == 'reset_zone',
                  enabled: !busy,
                  highlighted: true,
                  onPressed: () => _runZoneAction('reset_zone'),
                ),
              ),
            ],
          ),
          const SizedBox(height: 8),
          Row(
            children: [
              Expanded(
                child: _ZoneSyncButton(
                  icon: Icons.wb_sunny_outlined,
                  label: 'Sun -',
                  active: _zoneActionBusy == 'sun_down',
                  enabled: !busy,
                  onPressed: () => _runZoneAction('sun_down'),
                ),
              ),
              const SizedBox(width: 8),
              Expanded(
                child: _ZoneSyncButton(
                  icon: Icons.wb_sunny_rounded,
                  label: 'Sun +',
                  active: _zoneActionBusy == 'sun_up',
                  enabled: !busy,
                  onPressed: () => _runZoneAction('sun_up'),
                ),
              ),
            ],
          ),
          const SizedBox(height: 8),
          Align(
            alignment: Alignment.centerRight,
            child: TextButton.icon(
              onPressed: busy
                  ? null
                  : () => _runZoneAction(
                        'reset_zone_cascade',
                        confirmCascade: true,
                      ),
              icon: const Icon(Icons.restart_alt_rounded, size: 16),
              label: const Text('Reset zone and areas'),
              style: TextButton.styleFrom(
                foregroundColor: _dangerAccent,
                visualDensity: VisualDensity.compact,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _ExpertAreaDetailScreen extends StatefulWidget {
  final _ExpertZone zone;
  final List<_ExpertZone> zones;
  final _ExpertArea area;
  final _CircadianExpertClient? client;
  final _CurveReading seedReading;

  const _ExpertAreaDetailScreen({
    required this.zone,
    required this.zones,
    required this.area,
    required this.client,
    required this.seedReading,
  });

  @override
  State<_ExpertAreaDetailScreen> createState() =>
      _ExpertAreaDetailScreenState();
}

class _ExpertAreaDetailScreenState extends State<_ExpertAreaDetailScreen> {
  final Map<String, Object?> _pendingSettings = {};
  late _ExpertAreaSettings _settings = _ExpertAreaSettings.defaults();
  _ExpertAreaStatus? _status;
  List<_ExpertHistoryEntry> _historyEntries = const [];
  String? _historyHint;
  List<_ExpertLightRow> _lightRows = const [];
  List<_ExpertSectionOption> _sectionOptions = const [];
  List<_ExpertSectionAdjustRow> _sectionAdjustRows = const [];
  List<_ExpertScheduleParticipant> _scheduleParticipants = const [];
  List<_ExpertSectionTuneRow> _sectionTuneRows = const [];
  List<_ExpertControl> _areaControls = const [];
  List<String> _lightPresetOptions = const ['Standard'];
  List<_SliderPreviewPoint> _sliderPreviewPoints = const [];
  List<_ExpertFeedbackTargetChoice> _feedbackTargetChoices = const [];
  Map<String, String> _lightPurposeSnapshot = const {};
  final Map<String, String> _pendingLightPurposeChanges = {};
  _ExpertConfig _expertConfig = const _ExpertConfig.empty();
  int _areaControlDefaultPauseMinutes = 240;
  Timer? _settingsSaveDebounce;
  bool _loading = true;
  bool _saving = false;
  bool _actionBusy = false;
  bool _adjustDragging = false;
  bool? _frozenOverride;
  bool? _boostedOverride;
  bool _tuneSaving = false;
  bool _feedbackTargetSaving = false;
  bool _lightPurposeBulkSaving = false;
  String? _zoneSyncAction;
  String? _lightsSavingEntityId;
  String? _identifyingLightEntityId;
  String? _sectionsSavingKey;
  String? _participationSavingKey;
  String? _sectionAdjustSavingKey;
  String? _sectionTuneSavingKey;
  String? _controlsError;
  double? _draftBrightness;
  double? _draftKelvin;
  double? _draftPhaseHour;
  int? _draftLumenStep;
  int? _draftSolarStep;
  String _feedbackTarget = '';
  String? _error;

  @override
  void initState() {
    super.initState();
    unawaited(_loadArea());
  }

  @override
  void dispose() {
    final client = widget.client;
    if (client != null &&
        _modernExpertCanEditAreaSettings &&
        _pendingSettings.isNotEmpty) {
      final updates = Map<String, Object?>.from(_pendingSettings);
      _pendingSettings.clear();
      unawaited(client.saveAreaSettings(widget.area.id, updates));
    }
    _settingsSaveDebounce?.cancel();
    super.dispose();
  }

  Future<void> _loadArea({bool silent = false}) async {
    final client = widget.client;
    if (client == null) {
      setState(() {
        _loading = false;
        _error = 'Connect to RhythmOS to control this area.';
      });
      return;
    }

    if (!silent) {
      setState(() {
        _loading = true;
        _error = null;
      });
    }

    try {
      final results = await Future.wait<Object?>([
        client.fetchAreaStatus(widget.area.id),
        client.fetchAreaSettings(widget.area.id),
        client.fetchAreaHistory(widget.area.id, limit: 12),
        client.fetchAreaLightData(widget.area.id),
        client.fetchAreaNow(widget.area.id).catchError((
          Object error,
          StackTrace stackTrace,
        ) {
          debugPrint('CircadianExpert: area now load failed: $error');
          debugPrint('$stackTrace');
          return null;
        }),
        client.fetchAreaSliderPreview(widget.area.id).catchError((
          Object error,
          StackTrace stackTrace,
        ) {
          debugPrint('CircadianExpert: slider preview load failed: $error');
          debugPrint('$stackTrace');
          return <_SliderPreviewPoint>[];
        }),
        client.fetchAreaControls(widget.area.id).catchError((
          Object error,
          StackTrace stackTrace,
        ) {
          debugPrint('CircadianExpert: area controls load failed: $error');
          debugPrint('$stackTrace');
          return _ExpertAreaControlsLoad.unavailable(error.toString());
        }),
        client.fetchExpertConfig().catchError((
          Object error,
          StackTrace stackTrace,
        ) {
          debugPrint('CircadianExpert: expert config load failed: $error');
          debugPrint('$stackTrace');
          return const _ExpertConfig.empty();
        }),
      ]);
      if (!mounted) return;
      setState(() {
        final nextStatus = results[0] as _ExpertAreaStatus?;
        final history = results[2] as _ExpertAreaHistory;
        final lightData = results[3] as _ExpertAreaLightData;
        _status = nextStatus;
        _settings = results[1] as _ExpertAreaSettings;
        if (!_adjustDragging) _syncAdjustDrafts(nextStatus);
        if (!_tuneSaving) _syncTuneDrafts(nextStatus);
        _historyEntries = [
          if (results[4] case final _ExpertHistoryEntry nowEntry) nowEntry,
          ...history.entries,
        ];
        _historyHint = history.hint;
        _lightPurposeSnapshot = _snapshotLightPurposes(lightData.rows);
        _pendingLightPurposeChanges.removeWhere((entityId, purpose) {
          final serverPurpose = _lightPurposeSnapshot[entityId];
          return serverPurpose == null || serverPurpose == purpose;
        });
        _lightRows = _withPendingLightPurposes(lightData.rows);
        _sectionOptions = lightData.sections;
        _sectionAdjustRows = lightData.sectionAdjusts;
        _scheduleParticipants = lightData.participants;
        _sectionTuneRows = lightData.sectionTunes;
        _lightPresetOptions = lightData.presets;
        _feedbackTarget = lightData.feedbackTarget ?? '';
        _feedbackTargetChoices = lightData.feedbackTargetChoices;
        _sliderPreviewPoints = results[5] as List<_SliderPreviewPoint>;
        final controlsLoad = results[6] as _ExpertAreaControlsLoad;
        _areaControls = controlsLoad.controls;
        _areaControlDefaultPauseMinutes = controlsLoad.defaultPauseMinutes;
        _controlsError = controlsLoad.error;
        _expertConfig = results[7] as _ExpertConfig;
        _loading = false;
        _error = null;
      });
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: area load failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() {
        _loading = false;
        _error = 'Area server load failed: $error';
      });
    }
  }

  void _syncAdjustDrafts(_ExpertAreaStatus? status) {
    if (status == null) {
      _draftBrightness = widget.seedReading.brightness.toDouble();
      _draftKelvin = widget.seedReading.kelvin.toDouble();
      _draftPhaseHour = _normalizeHour(widget.zone.definition.wakeHour);
      return;
    }

    _draftBrightness = status.actualBrightness.toDouble();
    _draftKelvin = status.kelvin.toDouble();
    _draftPhaseHour = status.phaseTargetHour ??
        (status.phase == 'bed'
            ? widget.zone.definition.bedHour
            : widget.zone.definition.wakeHour);
  }

  void _syncTuneDrafts(_ExpertAreaStatus? status) {
    _draftLumenStep = _closestLumenStep(status?.areaFactor ?? 1.0);
    _draftSolarStep = _closestSolarStep(status?.naturalLightExposure ?? 0.0);
  }

  Map<String, String> _snapshotLightPurposes(List<_ExpertLightRow> rows) {
    return {
      for (final row in rows) row.entityId: row.purpose,
    };
  }

  int get _defaultFreezeDurationMinutes => _expertDurationMinutes(
        _expertConfig,
        'default_freeze_duration_minutes',
        fallback: 0,
      );

  int get _defaultBoostDurationMinutes => _expertDurationMinutes(
        _expertConfig,
        'default_boost_duration_minutes',
        fallback: 60,
      );

  int get _defaultPowerOffDurationMinutes => _expertDurationMinutes(
        _expertConfig,
        'default_power_off_duration_minutes',
        fallback: 60,
      );

  int get _rhythmCursorStepMinutes => _expertCursorStepMinutes(_expertConfig);

  List<String> get _areaDurationPresetValues => _durationPresetValues(
        _expertConfig,
        includeValues: [
          _defaultFreezeDurationMinutes.toString(),
          _defaultBoostDurationMinutes.toString(),
          _defaultPowerOffDurationMinutes.toString(),
        ],
      );

  List<_ExpertLightRow> _withPendingLightPurposes(
    List<_ExpertLightRow> rows,
  ) {
    if (_pendingLightPurposeChanges.isEmpty) return rows;
    return [
      for (final row in rows)
        row.copyWith(
          purpose: _pendingLightPurposeChanges[row.entityId] ?? row.purpose,
        ),
    ];
  }

  void _queueSettingsUpdate(Map<String, Object?> updates) {
    if (!_modernExpertCanEditAreaSettings) {
      _showSnack('This build is missing the Expert area settings API.');
      return;
    }
    _pendingSettings.addAll(updates);
    _settingsSaveDebounce?.cancel();
    setState(() {
      _settings = _settings.merged(updates);
      _saving = true;
    });
    _settingsSaveDebounce = Timer(const Duration(milliseconds: 450), () {
      final data = Map<String, Object?>.from(_pendingSettings);
      _pendingSettings.clear();
      unawaited(_commitSettings(data));
    });
  }

  Future<void> _commitSettings(Map<String, Object?> updates) async {
    if (updates.isEmpty) return;
    if (!_modernExpertCanEditAreaSettings) {
      if (mounted) setState(() => _saving = false);
      return;
    }
    final client = widget.client;
    if (client == null) {
      if (mounted) setState(() => _saving = false);
      return;
    }

    try {
      await client.saveAreaSettings(widget.area.id, updates);
      if (!mounted) return;
      setState(() => _saving = _pendingSettings.isNotEmpty);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: area settings save failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() {
        _saving = false;
        _error = 'Area settings save failed: $error';
      });
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('Area settings save failed: $error')),
      );
    }
  }

  Future<void> _runAreaAction(
    String action, {
    Map<String, Object?> extra = const {},
  }) async {
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before controlling this area.');
      return;
    }

    HapticFeedback.selectionClick();
    setState(() => _actionBusy = true);
    try {
      await client.runAreaAction(widget.area.id, action, extra);
      await _loadArea(silent: true);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: area action failed: $error');
      debugPrint('$stackTrace');
      _showSnack('Area action failed: $error');
    } finally {
      if (mounted) setState(() => _actionBusy = false);
    }
  }

  Future<void> _setAreaFrozen(bool enabled) async {
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before controlling this area.');
      return;
    }

    final previous = _frozenOverride;
    HapticFeedback.selectionClick();
    setState(() {
      _frozenOverride = enabled;
      _actionBusy = true;
    });
    try {
      await client.runAreaAction(
        widget.area.id,
        'freeze_toggle',
        {
          'enabled': enabled,
          if (enabled) 'duration_minutes': _defaultFreezeDurationMinutes,
        },
      );
      await _loadArea(silent: true);
      if (mounted) setState(() => _frozenOverride = enabled);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: area freeze failed: $error');
      debugPrint('$stackTrace');
      if (mounted) setState(() => _frozenOverride = previous);
      _showSnack('Area freeze failed: $error');
    } finally {
      if (mounted) setState(() => _actionBusy = false);
    }
  }

  Future<void> _setAreaBoosted(bool enabled) async {
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before controlling this area.');
      return;
    }

    final previous = _boostedOverride;
    HapticFeedback.selectionClick();
    setState(() {
      _boostedOverride = enabled;
      _actionBusy = true;
    });
    try {
      await client.runAreaAction(
        widget.area.id,
        enabled ? 'boost_on' : 'boost_off',
        {
          if (enabled) 'duration_minutes': _defaultBoostDurationMinutes,
        },
      );
      await _loadArea(silent: true);
      if (mounted) setState(() => _boostedOverride = enabled);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: area boost failed: $error');
      debugPrint('$stackTrace');
      if (mounted) setState(() => _boostedOverride = previous);
      _showSnack('Area boost failed: $error');
    } finally {
      if (mounted) setState(() => _actionBusy = false);
    }
  }

  int _currentAreaKelvin() {
    final minKelvin = (_status?.minKelvin ?? widget.zone.definition.minKelvin)
        .clamp(1500, 7000)
        .toInt();
    final maxKelvin = (_status?.maxKelvin ?? widget.zone.definition.maxKelvin)
        .clamp(1500, 7000)
        .toInt();
    final value = _draftKelvin ??
        _status?.kelvin.toDouble() ??
        widget.seedReading.kelvin.toDouble();
    return value.clamp(minKelvin.toDouble(), maxKelvin.toDouble()).round();
  }

  int _nextAreaKelvin(String action) {
    final minKelvin = (_status?.minKelvin ?? widget.zone.definition.minKelvin)
        .clamp(1500, 7000)
        .toInt();
    final maxKelvin = (_status?.maxKelvin ?? widget.zone.definition.maxKelvin)
        .clamp(1500, 7000)
        .toInt();
    final delta = action == 'color_up' ? _expertKelvinStep : -_expertKelvinStep;
    return (_currentAreaKelvin() + delta).clamp(minKelvin, maxKelvin).toInt();
  }

  Future<void> _runAdjustAction(
    String action, {
    Map<String, Object?> extra = const {},
  }) async {
    if (action == 'set_phase_time' && !_modernExpertCanSetPhaseTime) {
      _showSnack('This build is missing the Expert phase-time API.');
      return;
    }
    if ((action == 'reset_phase' ||
            action == 'reset_brightness_override' ||
            action == 'reset_color_override') &&
        !_modernExpertCanResetAreaAdjustments) {
      _showSnack('This build is missing the Expert adjustment reset API.');
      return;
    }
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before adjusting this area.');
      return;
    }

    HapticFeedback.selectionClick();
    setState(() {
      _actionBusy = true;
      _adjustDragging = false;
    });
    try {
      if (action == 'set_circadian') {
        final brightness = _intValue(
          extra['value'],
          fallback: (_draftBrightness ??
                  _status?.actualBrightness.toDouble() ??
                  widget.seedReading.brightness.toDouble())
              .round(),
        ).clamp(1, 100).toInt();
        setState(() => _draftBrightness = brightness.toDouble());
        await client.setAreaBrightness(widget.area.id, brightness);
      } else if (action == 'set_color_temperature') {
        final kelvin = _intValue(
          extra['kelvin'],
          fallback: _currentAreaKelvin(),
        ).clamp(1500, 7000).toInt();
        setState(() => _draftKelvin = kelvin.toDouble());
        await client.runAreaAction(
          widget.area.id,
          'set_color_temperature',
          {'kelvin': kelvin},
        );
      } else if (action == 'color_up' || action == 'color_down') {
        final kelvin = _nextAreaKelvin(action);
        setState(() => _draftKelvin = kelvin.toDouble());
        await client.runAreaAction(widget.area.id, action);
      } else {
        await client.runAreaAction(widget.area.id, action, extra);
      }
      await _loadArea(silent: true);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: area adjust failed: $error');
      debugPrint('$stackTrace');
      _showSnack('Area adjust failed: $error');
    } finally {
      if (mounted) {
        setState(() {
          _actionBusy = false;
          _adjustDragging = false;
        });
      }
    }
  }

  void _nudgeAreaPhase(int direction) {
    final base = _normalizeHour(
      _draftPhaseHour ??
          _status?.phaseTargetHour ??
          (_status?.phase == 'bed'
              ? widget.zone.definition.bedHour
              : widget.zone.definition.wakeHour),
    );
    final next = _nudgeExpertPhaseHour(
      base,
      direction,
      _rhythmCursorStepMinutes,
    );
    setState(() {
      _adjustDragging = true;
      _draftPhaseHour = next;
    });
    unawaited(
      _runAdjustAction(
        'set_phase_time',
        extra: {'target_time': next},
      ),
    );
  }

  Future<void> _runZoneSyncAction(String action) async {
    if ((action == 'glo_up' || action == 'glo_down') &&
        !_modernExpertCanSyncAreaToZone) {
      _showSnack('This build is missing the Expert zone sync API.');
      return;
    }
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before syncing this area.');
      return;
    }

    if (action == 'glo_reset') {
      final confirmed = await showDialog<bool>(
        context: context,
        builder: (context) => AlertDialog(
          backgroundColor: CelestialColors.backgroundCard,
          title: const Text(
            'Reset zone',
            style: TextStyle(color: CelestialColors.textPrimary),
          ),
          content: Text(
            'Reset "${widget.zone.name}" and its member areas?',
            style: const TextStyle(color: CelestialColors.textSecondary),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.of(context).pop(false),
              child: const Text('Cancel'),
            ),
            TextButton(
              onPressed: () => Navigator.of(context).pop(true),
              child: const Text(
                'Reset',
                style: TextStyle(color: _dangerAccent),
              ),
            ),
          ],
        ),
      );
      if (confirmed != true) return;
    } else if ((action == 'glo_up' || action == 'glo_down') &&
        _expertConfig.boolValue('confirm_zone_pushes', fallback: true)) {
      final confirmed = await _confirmZonePush(action);
      if (confirmed != true) return;
    }

    HapticFeedback.selectionClick();
    setState(() => _zoneSyncAction = action);
    try {
      if (action == 'glo_reset') {
        await client.runZoneAction(widget.zone.name, 'reset_zone_cascade');
      } else {
        await client.runAreaAction(widget.area.id, action);
      }
      await _loadArea(silent: true);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: zone sync failed: $error');
      debugPrint('$stackTrace');
      _showSnack('Zone sync failed: $error');
    } finally {
      if (mounted) setState(() => _zoneSyncAction = null);
    }
  }

  Future<bool?> _confirmZonePush(String action) {
    final isFullSend = action == 'glo_up';
    return showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: Text(
          isFullSend ? 'Full Send' : 'Pull zone',
          style: const TextStyle(color: CelestialColors.textPrimary),
        ),
        content: Text(
          isFullSend
              ? 'Push "${widget.area.name}" into "${widget.zone.name}"? Areas open to this zone may adjust.'
              : 'Pull "${widget.zone.name}" into "${widget.area.name}"?',
          style: const TextStyle(color: CelestialColors.textSecondary),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('Cancel'),
          ),
          TextButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: Text(isFullSend ? 'Full Send' : 'Pull'),
          ),
        ],
      ),
    );
  }

  Future<void> _saveTune({
    int? lumenStep,
    int? solarStep,
  }) async {
    if (!_modernExpertCanEditAreaTuning) {
      _showSnack('This build is missing the Expert area tuning API.');
      return;
    }
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before tuning this area.');
      return;
    }

    HapticFeedback.selectionClick();
    setState(() => _tuneSaving = true);
    try {
      await client.saveAreaTune(
        widget.area.id,
        brightnessFactor:
            lumenStep == null ? null : _lumenSteps[lumenStep].factor,
        naturalLightExposure:
            solarStep == null ? null : _solarSteps[solarStep].exposure,
      );
      await _loadArea(silent: true);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: area tune save failed: $error');
      debugPrint('$stackTrace');
      _showSnack('Area tune save failed: $error');
    } finally {
      if (mounted) setState(() => _tuneSaving = false);
    }
  }

  Future<void> _saveFeedbackTarget(String value) async {
    if (!_modernExpertCanEditFeedbackTargets) {
      _showSnack('This build is missing the Expert feedback-target API.');
      return;
    }
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before changing feedback target.');
      return;
    }

    final previous = _feedbackTarget;
    HapticFeedback.selectionClick();
    setState(() {
      _feedbackTarget = value;
      _feedbackTargetSaving = true;
    });
    try {
      await client.saveAreaFeedbackTarget(
        widget.area.id,
        value.isEmpty ? null : value,
      );
      await _loadArea(silent: true);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: feedback target save failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() => _feedbackTarget = previous);
      _showSnack('Feedback target save failed: $error');
    } finally {
      if (mounted) setState(() => _feedbackTargetSaving = false);
    }
  }

  void _stageLightPurpose(
    _ExpertLightRow light,
    String purpose,
  ) {
    if (!_modernExpertCanEditLightPurposes) {
      _showSnack('This build is missing the Expert light purpose API.');
      return;
    }
    HapticFeedback.selectionClick();
    final serverPurpose =
        _lightPurposeSnapshot[light.entityId] ?? light.purpose;
    setState(() {
      if (purpose == serverPurpose) {
        _pendingLightPurposeChanges.remove(light.entityId);
      } else {
        _pendingLightPurposeChanges[light.entityId] = purpose;
      }
      _lightRows = [
        for (final row in _lightRows)
          row.entityId == light.entityId ? row.copyWith(purpose: purpose) : row,
      ];
    });
  }

  void _cancelLightPurposeChanges() {
    if (_pendingLightPurposeChanges.isEmpty || _lightPurposeBulkSaving) return;
    HapticFeedback.selectionClick();
    setState(() {
      _pendingLightPurposeChanges.clear();
      _lightRows = [
        for (final row in _lightRows)
          row.copyWith(
            purpose: _lightPurposeSnapshot[row.entityId] ?? row.purpose,
          ),
      ];
    });
  }

  Future<void> _saveLightPurposeChanges() async {
    if (!_modernExpertCanEditLightPurposes) {
      _showSnack('This build is missing the Expert light purpose API.');
      return;
    }
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before changing light purposes.');
      return;
    }
    if (_pendingLightPurposeChanges.isEmpty || _lightPurposeBulkSaving) return;

    HapticFeedback.selectionClick();
    final changes = Map<String, String>.from(_pendingLightPurposeChanges);
    setState(() => _lightPurposeBulkSaving = true);
    try {
      await client.saveLightFiltersBulk(widget.area.id, changes);
      if (!mounted) return;
      setState(_pendingLightPurposeChanges.clear);
      await _loadArea(silent: true);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: light purpose save failed: $error');
      debugPrint('$stackTrace');
      if (mounted) _showSnack('Light purpose save failed: $error');
    } finally {
      if (mounted) setState(() => _lightPurposeBulkSaving = false);
    }
  }

  Future<void> _assignLightSection(
    _ExpertLightRow light,
    String? sectionId,
  ) async {
    if (!_modernExpertCanAssignLightSections) {
      _showSnack('This build is missing the Expert light-section API.');
      return;
    }
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before moving lights.');
      return;
    }

    HapticFeedback.selectionClick();
    final previousRows = _lightRows;
    final sectionName = _sectionNameForId(sectionId);
    setState(() {
      _lightsSavingEntityId = light.entityId;
      _lightRows = [
        for (final row in _lightRows)
          row.entityId == light.entityId
              ? row.copyWith(
                  sectionId: sectionId,
                  sectionName: sectionName,
                  clearSection: sectionId == null,
                )
              : row,
      ];
    });

    try {
      await client.assignLightToSection(
        areaId: widget.area.id,
        entityId: light.entityId,
        sectionId: sectionId,
      );
      await _loadArea(silent: true);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: light section save failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() => _lightRows = previousRows);
      _showSnack('Light section save failed: $error');
    } finally {
      if (mounted) setState(() => _lightsSavingEntityId = null);
    }
  }

  Future<void> _identifyLight(_ExpertLightRow light) async {
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before identifying lights.');
      return;
    }
    HapticFeedback.selectionClick();
    setState(() => _identifyingLightEntityId = light.entityId);
    try {
      await client.flashLight(light.entityId);
      _showSnack('Identified ${light.name}.');
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: identify light failed: $error');
      debugPrint('$stackTrace');
      _showSnack('Identify light failed: $error');
    } finally {
      if (mounted) setState(() => _identifyingLightEntityId = null);
    }
  }

  Future<void> _createSection() async {
    if (!_modernExpertCanEditSections) {
      _showSnack('This build is missing the Expert section editing API.');
      return;
    }
    final name = await _promptSectionName(title: 'New section');
    if (name == null) return;

    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before creating sections.');
      return;
    }

    HapticFeedback.selectionClick();
    setState(() => _sectionsSavingKey = '__create__');
    try {
      await client.createSection(widget.area.id, name);
      await _loadArea(silent: true);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: section create failed: $error');
      debugPrint('$stackTrace');
      _showSnack('Section create failed: $error');
    } finally {
      if (mounted) setState(() => _sectionsSavingKey = null);
    }
  }

  Future<void> _renameSection(_ExpertSectionOption section) async {
    if (!_modernExpertCanEditSections) {
      _showSnack('This build is missing the Expert section editing API.');
      return;
    }
    final name = await _promptSectionName(
      title: 'Rename section',
      initialValue: section.name,
    );
    if (name == null || name == section.name) return;

    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before renaming sections.');
      return;
    }

    HapticFeedback.selectionClick();
    setState(() => _sectionsSavingKey = section.id);
    try {
      await client.renameSection(section.id, name);
      await _loadArea(silent: true);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: section rename failed: $error');
      debugPrint('$stackTrace');
      _showSnack('Section rename failed: $error');
    } finally {
      if (mounted) setState(() => _sectionsSavingKey = null);
    }
  }

  Future<void> _deleteSection(_ExpertSectionOption section) async {
    if (!_modernExpertCanEditSections) {
      _showSnack('This build is missing the Expert section editing API.');
      return;
    }
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: const Text(
          'Delete section',
          style: TextStyle(color: CelestialColors.textPrimary),
        ),
        content: Text(
          'Move lights in "${section.name}" back to Main?',
          style: const TextStyle(color: CelestialColors.textSecondary),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('Cancel'),
          ),
          TextButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text('Delete'),
          ),
        ],
      ),
    );
    if (confirmed != true) return;

    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before deleting sections.');
      return;
    }

    HapticFeedback.selectionClick();
    setState(() => _sectionsSavingKey = section.id);
    try {
      await client.deleteSection(section.id);
      await _loadArea(silent: true);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: section delete failed: $error');
      debugPrint('$stackTrace');
      _showSnack('Section delete failed: $error');
    } finally {
      if (mounted) setState(() => _sectionsSavingKey = null);
    }
  }

  Future<String?> _promptSectionName({
    required String title,
    String initialValue = '',
  }) async {
    final controller = TextEditingController(text: initialValue);
    final value = await showDialog<String>(
      context: context,
      builder: (context) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: Text(
          title,
          style: const TextStyle(color: CelestialColors.textPrimary),
        ),
        content: TextField(
          controller: controller,
          autofocus: true,
          style: const TextStyle(color: CelestialColors.textPrimary),
          decoration: const InputDecoration(
            labelText: 'Section name',
            labelStyle: TextStyle(color: CelestialColors.textSecondary),
            enabledBorder: UnderlineInputBorder(
              borderSide: BorderSide(color: CelestialColors.orbitRing),
            ),
            focusedBorder: UnderlineInputBorder(
              borderSide: BorderSide(color: _expertAccent),
            ),
          ),
          onSubmitted: (value) => Navigator.of(context).pop(value),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(),
            child: const Text('Cancel'),
          ),
          TextButton(
            onPressed: () => Navigator.of(context).pop(controller.text),
            child: const Text('Save'),
          ),
        ],
      ),
    );
    controller.dispose();
    final trimmed = value?.trim();
    return trimmed == null || trimmed.isEmpty ? null : trimmed;
  }

  String _sectionNameForId(String? sectionId) {
    if (sectionId == null) return 'Main section';
    for (final section in _sectionOptions) {
      if (section.id == sectionId) return section.name;
    }
    return sectionId;
  }

  Future<void> _saveScheduleParticipation(
    _ExpertScheduleParticipant participant, {
    required bool autoOn,
    required bool value,
  }) async {
    if (!_modernExpertCanEditScheduleParticipation) {
      _showSnack('This build is missing the Expert section schedule API.');
      return;
    }
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before changing section schedule.');
      return;
    }

    final key = '${participant.id}:${autoOn ? 'on' : 'off'}';
    final previous = _scheduleParticipants;
    HapticFeedback.selectionClick();
    setState(() {
      _participationSavingKey = key;
      _scheduleParticipants = [
        for (final item in _scheduleParticipants)
          item.id == participant.id
              ? item.copyWith(
                  participatesInAutoOn: autoOn ? value : null,
                  participatesInAutoOff: autoOn ? null : value,
                )
              : item,
      ];
    });

    try {
      await client.saveScheduleParticipation(
        areaId: widget.area.id,
        participantId: participant.id,
        isMain: participant.isMain,
        participatesInAutoOn: autoOn ? value : null,
        participatesInAutoOff: autoOn ? null : value,
      );
      await _loadArea(silent: true);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: schedule participation save failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() => _scheduleParticipants = previous);
      _showSnack('Section schedule save failed: $error');
    } finally {
      if (mounted) setState(() => _participationSavingKey = null);
    }
  }

  Future<void> _saveSectionTune(
    _ExpertSectionTuneRow row, {
    required String dimension,
    required double? value,
  }) async {
    if (!_modernExpertCanEditSectionTuning) {
      _showSnack('This build is missing the Expert section tuning API.');
      return;
    }
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before tuning sections.');
      return;
    }

    final key = '${row.id}:$dimension';
    final previous = _sectionTuneRows;
    HapticFeedback.selectionClick();
    setState(() {
      _sectionTuneSavingKey = key;
      _sectionTuneRows = [
        for (final item in _sectionTuneRows)
          item.id == row.id
              ? item.copyWith(
                  balance: dimension == 'balance' ? value : null,
                  sunDimming: dimension == 'sun_dimming' ? value : null,
                  clearBalance: dimension == 'balance' && value == null,
                  clearSunDimming: dimension == 'sun_dimming' && value == null,
                )
              : item,
      ];
    });

    try {
      await client.saveSectionTune(
        areaId: widget.area.id,
        sectionId: row.id,
        isMain: row.isMain,
        dimension: dimension,
        value: value,
      );
      await _loadArea(silent: true);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: section tune save failed: $error');
      debugPrint('$stackTrace');
      if (!mounted) return;
      setState(() => _sectionTuneRows = previous);
      _showSnack('Section tune save failed: $error');
    } finally {
      if (mounted) setState(() => _sectionTuneSavingKey = null);
    }
  }

  Future<void> _toggleSectionPower(_ExpertSectionAdjustRow row) async {
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before controlling sections.');
      return;
    }

    final key = '${row.id}:power';
    HapticFeedback.selectionClick();
    setState(() => _sectionAdjustSavingKey = key);
    try {
      await client.toggleSectionPower(
        areaId: widget.area.id,
        sectionId: row.id,
        isMain: row.isMain,
      );
      await _loadArea(silent: true);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: section power failed: $error');
      debugPrint('$stackTrace');
      _showSnack('Section power failed: $error');
    } finally {
      if (mounted) setState(() => _sectionAdjustSavingKey = null);
    }
  }

  Future<void> _saveSectionBrightness(
    _ExpertSectionAdjustRow row,
    double? value,
    double areaBrightness,
  ) async {
    if (value == null && !_modernExpertCanClearSectionBrightness) {
      _showSnack(
        'This build is missing the Expert section brightness-clear API.',
      );
      return;
    }
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before adjusting sections.');
      return;
    }

    final key = '${row.id}:brightness';
    HapticFeedback.selectionClick();
    setState(() => _sectionAdjustSavingKey = key);
    try {
      await client.saveSectionBrightness(
        areaId: widget.area.id,
        sectionId: row.id,
        isMain: row.isMain,
        areaBrightness: areaBrightness,
        value: value,
      );
      await _loadArea(silent: true);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: section brightness failed: $error');
      debugPrint('$stackTrace');
      _showSnack('Section brightness failed: $error');
    } finally {
      if (mounted) setState(() => _sectionAdjustSavingKey = null);
    }
  }

  Future<void> _stepSectionBrightness(
    _ExpertSectionAdjustRow row,
    String direction,
  ) async {
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before adjusting sections.');
      return;
    }

    final key = '${row.id}:step_$direction';
    HapticFeedback.selectionClick();
    setState(() => _sectionAdjustSavingKey = key);
    try {
      await client.stepSectionBrightness(
        areaId: widget.area.id,
        sectionId: row.id,
        isMain: row.isMain,
        direction: direction,
      );
      await _loadArea(silent: true);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: section brightness step failed: $error');
      debugPrint('$stackTrace');
      _showSnack('Section brightness step failed: $error');
    } finally {
      if (mounted) setState(() => _sectionAdjustSavingKey = null);
    }
  }

  Future<void> _setSectionAutoOff(
    _ExpertSectionAdjustRow row,
    int? durationMinutes,
  ) async {
    final client = widget.client;
    if (client == null) {
      _showSnack('Connect to RhythmOS before setting section timers.');
      return;
    }

    final key = '${row.id}:auto_off';
    HapticFeedback.selectionClick();
    setState(() => _sectionAdjustSavingKey = key);
    try {
      await client.setSectionAutoOff(
        areaId: widget.area.id,
        sectionId: row.id,
        isMain: row.isMain,
        durationMinutes: durationMinutes,
      );
      await _loadArea(silent: true);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: section auto-off failed: $error');
      debugPrint('$stackTrace');
      _showSnack('Section auto-off failed: $error');
    } finally {
      if (mounted) setState(() => _sectionAdjustSavingKey = null);
    }
  }

  void _updateScheduleDays(
    String prefix,
    List<int> current,
    int day, {
    bool secondary = false,
  }) {
    final next = current.contains(day)
        ? current.where((value) => value != day).toList()
        : [...current, day];
    next.sort();
    if (secondary) {
      _queueSettingsUpdate({'${prefix}_days_2': next});
    } else {
      _queueSettingsUpdate({
        '${prefix}_days': next,
        '${prefix}_days_1': next,
      });
    }
  }

  Future<void> _pickScheduleTime(
    String key,
    double? current,
  ) async {
    final initial = _timeOfDayFromHour(current ?? _decimalNow(TimeOfDay.now()));
    final picked = await showTimePicker(
      context: context,
      initialTime: initial,
      builder: (context, child) {
        return Theme(
          data: Theme.of(context).copyWith(
            colorScheme: const ColorScheme.dark(
              primary: _expertAccent,
              surface: _panelColor,
            ),
          ),
          child: child!,
        );
      },
    );
    if (picked == null) return;
    _queueSettingsUpdate({
      key: _roundQuarter(picked.hour + picked.minute / 60),
    });
  }

  Future<void> _openAutoScheduleOverride({
    required String prefix,
    required String title,
    required double? seedTime,
  }) async {
    final result = await showModalBottomSheet<Map<String, Object?>>(
      context: context,
      isScrollControlled: true,
      backgroundColor: Colors.transparent,
      builder: (context) => _AutoScheduleOverrideSheet(
        title: title,
        initialTime: seedTime ?? (prefix == 'auto_on' ? 9.0 : 23.0),
      ),
    );
    if (result == null) return;
    HapticFeedback.selectionClick();
    _queueSettingsUpdate({'${prefix}_override': result});
  }

  void _clearAutoScheduleOverride(String prefix) {
    HapticFeedback.selectionClick();
    _queueSettingsUpdate({'${prefix}_override': null});
  }

  Future<void> _toggleCircadian() async {
    final next = !(_status?.isCircadian ?? true);
    await _runAreaAction(next ? 'circadian_on' : 'circadian_off');
  }

  Future<void> _openControl(_ExpertControl control) async {
    await Navigator.of(context).push(
      MaterialPageRoute(
        builder: (_) => _ExpertControlDetailScreen(
          client: widget.client,
          initialControl: control,
          zones: _zonesWithCurrent(widget.zones, widget.zone),
          defaultPauseMinutes: _areaControlDefaultPauseMinutes,
        ),
      ),
    );
    if (mounted) unawaited(_loadArea(silent: true));
  }

  void _showSnack(String message) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(content: Text(message)),
    );
  }

  @override
  Widget build(BuildContext context) {
    final status = _status;
    final isOn = status?.isOn ?? true;
    final isCircadian = status?.isCircadian ?? true;
    final frozen = _frozenOverride ?? status?.frozen ?? false;
    final boosted = _boostedOverride ?? status?.boosted ?? false;
    final brightness = status?.actualBrightness ??
        status?.brightness ??
        widget.seedReading.brightness;
    final kelvin = status?.kelvin ?? widget.seedReading.kelvin;
    final tint = _cctToColor(kelvin);
    final headerInk = _readableOn(tint);

    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            AnimatedContainer(
              duration: const Duration(milliseconds: 180),
              decoration: BoxDecoration(
                color: Color.lerp(_panelColor, tint, isOn ? 0.28 : 0.05),
                border: const Border(
                  bottom: BorderSide(color: CelestialColors.orbitRing),
                ),
                borderRadius: const BorderRadius.only(
                  bottomLeft: Radius.circular(12),
                  bottomRight: Radius.circular(12),
                ),
              ),
              padding: const EdgeInsets.fromLTRB(12, 10, 12, 12),
              child: Column(
                children: [
                  Row(
                    children: [
                      _BackButton(
                        onPressed: () => Navigator.of(context).pop(),
                        ink: headerInk,
                        background: Colors.black.withValues(alpha: 0.18),
                      ),
                      const SizedBox(width: 10),
                      Expanded(
                        child: Text(
                          widget.area.name,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                            color: headerInk,
                            fontSize: 22,
                            fontWeight: FontWeight.w700,
                          ),
                        ),
                      ),
                      Text(
                        isOn ? '$brightness% / $kelvin K' : 'Off',
                        style: TextStyle(
                          color: headerInk.withValues(alpha: isOn ? 1 : 0.6),
                          fontSize: 18,
                          fontWeight: FontWeight.w700,
                          fontFeatures: const [FontFeature.tabularFigures()],
                        ),
                      ),
                    ],
                  ),
                  Padding(
                    padding: const EdgeInsets.only(left: 50, top: 2),
                    child: Row(
                      children: [
                        Expanded(
                          child: Text(
                            'in ${widget.zone.name}',
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(
                              color: headerInk.withValues(alpha: 0.75),
                              fontSize: 12,
                              fontWeight: FontWeight.w600,
                            ),
                          ),
                        ),
                        IconButton(
                          onPressed: _loading ? null : () => _loadArea(),
                          tooltip: 'Refresh area',
                          visualDensity: VisualDensity.compact,
                          icon: _loading
                              ? SizedBox(
                                  width: 18,
                                  height: 18,
                                  child: CircularProgressIndicator(
                                    strokeWidth: 2,
                                    color: headerInk,
                                  ),
                                )
                              : const Icon(Icons.refresh_rounded),
                          color: headerInk,
                        ),
                      ],
                    ),
                  ),
                ],
              ),
            ),
            Expanded(
              child: ListView(
                padding: const EdgeInsets.fromLTRB(16, 14, 16, 28),
                children: [
                  _buildActionPanel(
                    isOn: isOn,
                    frozen: frozen,
                    boosted: boosted,
                    isCircadian: isCircadian,
                  ),
                  const SizedBox(height: 12),
                  _buildZoneSyncPanel(status),
                  const SizedBox(height: 12),
                  _buildAdjustPanel(status),
                  const SizedBox(height: 12),
                  _buildStatusPanel(status),
                  const SizedBox(height: 12),
                  _buildTunePanel(status),
                  const SizedBox(height: 12),
                  _buildLightsPanel(),
                  const SizedBox(height: 12),
                  _buildControlsPanel(),
                  const SizedBox(height: 12),
                  _buildMotionPanel(),
                  const SizedBox(height: 12),
                  _buildSchedulePanel(
                    title: 'Auto On',
                    prefix: 'auto_on',
                    enabled: _settings.autoOnEnabled,
                    source: _settings.autoOnSource,
                    offset: _settings.autoOnOffset,
                    fade: _settings.autoOnFade,
                    nextValue: status?.nextAutoOn,
                    sourceOptions: const ['sunset', 'sunrise', 'custom'],
                  ),
                  const SizedBox(height: 12),
                  _buildSchedulePanel(
                    title: 'Auto Off',
                    prefix: 'auto_off',
                    enabled: _settings.autoOffEnabled,
                    source: _settings.autoOffSource,
                    offset: _settings.autoOffOffset,
                    fade: _settings.autoOffFade,
                    nextValue: status?.nextAutoOff,
                    sourceOptions: const ['sunrise', 'sunset', 'custom'],
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildActionPanel({
    required bool isOn,
    required bool frozen,
    required bool boosted,
    required bool isCircadian,
  }) {
    return _ExpertPanel(
      padding: const EdgeInsets.all(12),
      child: LayoutBuilder(
        builder: (context, constraints) {
          final tileWidth = (constraints.maxWidth - 24) / 4;
          return Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [
              SizedBox(
                width: tileWidth,
                child: _AreaActionTile(
                  icon: Icons.power_settings_new_rounded,
                  label: 'Power',
                  value: isOn ? 'on' : 'off',
                  active: isOn,
                  busy: _actionBusy,
                  onTap: () => _runAreaAction('lights_toggle'),
                ),
              ),
              SizedBox(
                width: tileWidth,
                child: _AreaActionTile(
                  icon: Icons.pause_circle_outline_rounded,
                  label: 'Freeze',
                  value: frozen ? 'held' : 'auto',
                  active: frozen,
                  busy: _actionBusy,
                  onTap: () => _setAreaFrozen(!frozen),
                ),
              ),
              SizedBox(
                width: tileWidth,
                child: _AreaActionTile(
                  icon: Icons.flash_on_rounded,
                  label: 'Boost',
                  value: boosted ? 'on' : 'normal',
                  active: boosted,
                  busy: _actionBusy,
                  onTap: () => _setAreaBoosted(!boosted),
                ),
              ),
              SizedBox(
                width: tileWidth,
                child: _AreaActionTile(
                  icon: Icons.wb_twilight_rounded,
                  label: 'Curve',
                  value: isCircadian ? 'on' : 'off',
                  active: isCircadian,
                  busy: _actionBusy,
                  onTap: _toggleCircadian,
                ),
              ),
            ],
          );
        },
      ),
    );
  }

  Widget _buildAdjustPanel(_ExpertAreaStatus? status) {
    final minBrightness =
        (status?.minBrightness ?? widget.zone.definition.minBrightness)
            .clamp(0, 100)
            .toDouble();
    final maxBrightness =
        (status?.maxBrightness ?? widget.zone.definition.maxBrightness)
            .clamp(1, 100)
            .toDouble();
    final minKelvin = (status?.minKelvin ?? widget.zone.definition.minKelvin)
        .clamp(1500, 7000)
        .toDouble();
    final maxKelvin = (status?.maxKelvin ?? widget.zone.definition.maxKelvin)
        .clamp(1500, 7000)
        .toDouble();
    final brightness = (_draftBrightness ??
            status?.actualBrightness.toDouble() ??
            widget.seedReading.brightness.toDouble())
        .clamp(minBrightness, maxBrightness)
        .toDouble();
    final kelvin = (_draftKelvin ??
            status?.kelvin.toDouble() ??
            widget.seedReading.kelvin.toDouble())
        .clamp(minKelvin, maxKelvin)
        .toDouble();
    final phaseHour = _normalizeHour(
      _draftPhaseHour ??
          status?.phaseTargetHour ??
          (status?.phase == 'bed'
              ? widget.zone.definition.bedHour
              : widget.zone.definition.wakeHour),
    );
    final phaseLabel = status?.phase == 'bed' ? 'Bed time' : 'Wake time';

    return _ExpertPanel(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              const Expanded(
                child: Text(
                  'Adjust',
                  style: TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 16,
                    fontWeight: FontWeight.w700,
                  ),
                ),
              ),
              _CompactTextButton(
                label: 'Reset all',
                color: status?.hasDivergence == true
                    ? _changedAccent
                    : CelestialColors.textSecondary,
                onPressed:
                    _actionBusy ? null : () => _runAdjustAction('glo_reset'),
              ),
            ],
          ),
          const SizedBox(height: 8),
          _AdjustSliderRow(
            label: phaseLabel,
            value: phaseHour,
            min: 0,
            max: 24,
            divisions: 96,
            display: _formatHour(phaseHour),
            dirty: status?.hasPhaseShift == true,
            sliderEnabled: _modernExpertCanSetPhaseTime,
            onChanged: (value) {
              setState(() {
                _adjustDragging = true;
                _draftPhaseHour = _normalizeHour(value);
              });
            },
            onChangeEnd: (value) {
              unawaited(
                _runAdjustAction(
                  'set_phase_time',
                  extra: {'target_time': _roundQuarter(value)},
                ),
              );
            },
            onDecrement: () => _nudgeAreaPhase(-1),
            onIncrement: () => _nudgeAreaPhase(1),
            onReset: status?.hasPhaseShift == true &&
                    _modernExpertCanResetAreaAdjustments
                ? () => _runAdjustAction('reset_phase')
                : null,
          ),
          _AdjustSliderRow(
            label: 'Brightness',
            value: brightness,
            min: minBrightness,
            max: maxBrightness,
            divisions: (maxBrightness - minBrightness).round().clamp(1, 100),
            display: '${brightness.round()}%',
            dirty: status?.hasBrightnessOverride == true,
            previewPoints: _sliderPreviewPoints,
            onChanged: (value) {
              setState(() {
                _adjustDragging = true;
                _draftBrightness = value;
              });
            },
            onChangeEnd: (value) {
              unawaited(
                _runAdjustAction(
                  'set_circadian',
                  extra: {'value': value.round()},
                ),
              );
            },
            onDecrement: () => _runAdjustAction('bright_down'),
            onIncrement: () => _runAdjustAction('bright_up'),
            onReset: status?.hasBrightnessOverride == true &&
                    _modernExpertCanResetAreaAdjustments
                ? () => _runAdjustAction('reset_brightness_override')
                : null,
          ),
          if (_sectionAdjustRows.isNotEmpty)
            _SectionAdjustRows(
              rows: _sectionAdjustRows,
              areaBrightness: brightness,
              durationPresetValues: _areaDurationPresetValues,
              defaultAutoOffMinutes: _defaultPowerOffDurationMinutes,
              savingKey: _sectionAdjustSavingKey,
              canClearBrightnessOverride:
                  _modernExpertCanClearSectionBrightness,
              onPower: (row) => unawaited(_toggleSectionPower(row)),
              onBrightness: (row, value) =>
                  unawaited(_saveSectionBrightness(row, value, brightness)),
              onStep: (row, direction) =>
                  unawaited(_stepSectionBrightness(row, direction)),
              onAutoOff: (row, durationMinutes) =>
                  unawaited(_setSectionAutoOff(row, durationMinutes)),
            ),
          _AdjustSliderRow(
            label: 'Color',
            value: kelvin,
            min: minKelvin,
            max: maxKelvin,
            divisions: ((maxKelvin - minKelvin) / 50).round().clamp(1, 120),
            display: '${kelvin.round()} K',
            dirty: status?.hasColorOverride == true,
            onChanged: (value) {
              setState(() {
                _adjustDragging = true;
                _draftKelvin = value;
              });
            },
            onChangeEnd: (value) {
              unawaited(
                _runAdjustAction(
                  'set_color_temperature',
                  extra: {'kelvin': value.round()},
                ),
              );
            },
            onDecrement: () => _runAdjustAction('color_down'),
            onIncrement: () => _runAdjustAction('color_up'),
            onReset: status?.hasColorOverride == true &&
                    _modernExpertCanResetAreaAdjustments
                ? () => _runAdjustAction('reset_color_override')
                : null,
          ),
        ],
      ),
    );
  }

  Widget _buildZoneSyncPanel(_ExpertAreaStatus? status) {
    final busy = _zoneSyncAction != null || _actionBusy;
    final diverged = status?.hasDivergence == true;
    return _ExpertPanel(
      padding: const EdgeInsets.all(12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              const Expanded(
                child: Text(
                  'Zone sync',
                  style: TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 15,
                    fontWeight: FontWeight.w800,
                  ),
                ),
              ),
              if (busy)
                const SizedBox(
                  width: 16,
                  height: 16,
                  child: CircularProgressIndicator(strokeWidth: 2),
                ),
            ],
          ),
          const SizedBox(height: 10),
          Row(
            children: [
              Expanded(
                child: _ZoneSyncButton(
                  icon: Icons.upload_rounded,
                  label: 'Full Send',
                  active: _zoneSyncAction == 'glo_up',
                  enabled: !busy && _modernExpertCanSyncAreaToZone,
                  onPressed: () => _runZoneSyncAction('glo_up'),
                ),
              ),
              const SizedBox(width: 8),
              Expanded(
                child: _ZoneSyncButton(
                  icon: Icons.download_rounded,
                  label: 'Pull Zone',
                  active: _zoneSyncAction == 'glo_down',
                  enabled: !busy && _modernExpertCanSyncAreaToZone,
                  highlighted: diverged,
                  onPressed: () => _runZoneSyncAction('glo_down'),
                ),
              ),
              const SizedBox(width: 8),
              Expanded(
                child: _ZoneSyncButton(
                  icon: Icons.restart_alt_rounded,
                  label: 'Reset Zone',
                  active: _zoneSyncAction == 'glo_reset',
                  enabled: !busy,
                  danger: true,
                  onPressed: () => _runZoneSyncAction('glo_reset'),
                ),
              ),
            ],
          ),
        ],
      ),
    );
  }

  Widget _buildStatusPanel(_ExpertAreaStatus? status) {
    final metrics = <Widget>[
      _AreaMetricChip(
        icon: Icons.light_mode_rounded,
        label: 'target',
        value: '${status?.actualBrightness ?? widget.seedReading.brightness}%',
      ),
      _AreaMetricChip(
        icon: Icons.thermostat_rounded,
        label: 'color',
        value: '${status?.kelvin ?? widget.seedReading.kelvin} K',
      ),
      _AreaMetricChip(
        icon: Icons.tune_rounded,
        label: 'range',
        value: status == null
            ? '${widget.zone.definition.minBrightness}-${widget.zone.definition.maxBrightness}%'
            : '${status.minBrightness}-${status.maxBrightness}%',
      ),
      _AreaMetricChip(
        icon: Icons.balance_rounded,
        label: 'factor',
        value: '${((status?.areaFactor ?? 1) * 100).round()}%',
      ),
    ];

    final phase = status?.phase;
    final nextAutoOff = status?.nextAutoOff;
    if (phase != null) {
      metrics.add(
        _AreaMetricChip(
          icon: Icons.timeline_rounded,
          label: 'phase',
          value: phase,
        ),
      );
    }
    if (nextAutoOff != null) {
      metrics.add(
        _AreaMetricChip(
          icon: Icons.timer_off_rounded,
          label: 'off',
          value: _shortServerTime(nextAutoOff),
        ),
      );
    }

    return _ExpertPanel(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              const Expanded(
                child: Text(
                  'Server State',
                  style: TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 16,
                    fontWeight: FontWeight.w700,
                  ),
                ),
              ),
              if (_loading)
                const SizedBox(
                  width: 16,
                  height: 16,
                  child: CircularProgressIndicator(strokeWidth: 2),
                ),
            ],
          ),
          if (_error != null) ...[
            const SizedBox(height: 8),
            Text(
              _error!,
              style: const TextStyle(
                color: _dangerAccent,
                fontSize: 12,
                fontWeight: FontWeight.w600,
              ),
            ),
          ],
          const SizedBox(height: 10),
          Wrap(spacing: 8, runSpacing: 8, children: metrics),
        ],
      ),
    );
  }

  Widget _buildTunePanel(_ExpertAreaStatus? status) {
    final lumenStep =
        (_draftLumenStep ?? _closestLumenStep(status?.areaFactor ?? 1.0))
            .clamp(0, _lumenSteps.length - 1)
            .toInt();
    final solarStep = (_draftSolarStep ??
            _closestSolarStep(status?.naturalLightExposure ?? 0.0))
        .clamp(0, _solarSteps.length - 1)
        .toInt();
    final lumen = _lumenSteps[lumenStep];
    final solar = _solarSteps[solarStep];
    final sunFactor = status?.sunBrightFactor;
    final outdoor = status?.outdoorNormalized;

    return _ExpertPanel(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              const Expanded(
                child: Text(
                  'Tune',
                  style: TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 16,
                    fontWeight: FontWeight.w700,
                  ),
                ),
              ),
              if (_tuneSaving)
                const SizedBox(
                  width: 16,
                  height: 16,
                  child: CircularProgressIndicator(strokeWidth: 2),
                ),
            ],
          ),
          if (outdoor != null || sunFactor != null) ...[
            const SizedBox(height: 8),
            Wrap(
              spacing: 8,
              runSpacing: 8,
              children: [
                if (outdoor != null)
                  _AreaMetricChip(
                    icon: Icons.wb_sunny_outlined,
                    label: 'outside',
                    value: '${(outdoor * 100).round()}%',
                  ),
                if (sunFactor != null)
                  _AreaMetricChip(
                    icon: Icons.filter_drama_rounded,
                    label: 'sun trim',
                    value: '${((1 - sunFactor) * 100).round().clamp(0, 100)}%',
                  ),
              ],
            ),
          ],
          _TuneStepSlider(
            label: 'Balance',
            value: lumenStep,
            steps: _lumenSteps.map((step) => step.label).toList(),
            display: '${lumen.label} · ${(lumen.factor * 100).round()}%',
            saving: _tuneSaving,
            enabled: _modernExpertCanEditAreaTuning,
            onChanged: (value) => setState(() => _draftLumenStep = value),
            onChangeEnd: (value) => unawaited(_saveTune(lumenStep: value)),
          ),
          _TuneStepSlider(
            label: 'Sun dimming',
            value: solarStep,
            steps: _solarSteps.map((step) => step.label).toList(),
            display: '${solar.label} · ${(solar.exposure * 100).round()}%',
            saving: _tuneSaving,
            enabled: _modernExpertCanEditAreaTuning,
            onChanged: (value) => setState(() => _draftSolarStep = value),
            onChangeEnd: (value) => unawaited(_saveTune(solarStep: value)),
          ),
          if (_feedbackTargetChoices.isNotEmpty)
            _FeedbackTargetRow(
              value: _feedbackTarget,
              choices: _feedbackTargetChoices,
              saving: _feedbackTargetSaving,
              enabled: _modernExpertCanEditFeedbackTargets,
              onChanged: (value) => unawaited(_saveFeedbackTarget(value)),
            ),
          if (_sectionTuneRows.isNotEmpty)
            _SectionTuneRows(
              rows: _sectionTuneRows,
              areaLumenStep: lumenStep,
              areaSolarStep: solarStep,
              savingKey: _sectionTuneSavingKey,
              enabled: _modernExpertCanEditSectionTuning,
              onChanged: (row, dimension, value) => unawaited(
                _saveSectionTune(
                  row,
                  dimension: dimension,
                  value: value,
                ),
              ),
            ),
          const SizedBox(height: 12),
          _HistoryPreview(
            entries: _historyEntries,
            hint: _historyHint,
            onRefresh: () => _loadArea(silent: true),
          ),
        ],
      ),
    );
  }

  Widget _buildLightsPanel() {
    final pendingPurposeCount = _pendingLightPurposeChanges.length;
    return _ExpertPanel(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              const Expanded(
                child: Text(
                  'Lights',
                  style: TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 16,
                    fontWeight: FontWeight.w700,
                  ),
                ),
              ),
              Text(
                _lightRows.isEmpty ? '' : '${_lightRows.length}',
                style: const TextStyle(
                  color: CelestialColors.textSecondary,
                  fontSize: 13,
                  fontWeight: FontWeight.w800,
                  fontFeatures: [FontFeature.tabularFigures()],
                ),
              ),
              if (pendingPurposeCount > 0) ...[
                const SizedBox(width: 10),
                Text(
                  '$pendingPurposeCount changed',
                  style: const TextStyle(
                    color: _expertAccent,
                    fontSize: 11,
                    fontWeight: FontWeight.w800,
                    fontFeatures: [FontFeature.tabularFigures()],
                  ),
                ),
                const SizedBox(width: 6),
                Tooltip(
                  message: 'Cancel purpose changes',
                  child: _SmallSquareButton(
                    icon: Icons.close_rounded,
                    onPressed: _lightPurposeBulkSaving ||
                            !_modernExpertCanEditLightPurposes
                        ? null
                        : _cancelLightPurposeChanges,
                  ),
                ),
                const SizedBox(width: 4),
                Tooltip(
                  message: 'Save purpose changes',
                  child: _SmallSquareButton(
                    icon: Icons.check_rounded,
                    onPressed: _lightPurposeBulkSaving ||
                            !_modernExpertCanEditLightPurposes
                        ? null
                        : () => unawaited(_saveLightPurposeChanges()),
                  ),
                ),
              ],
              if (_lightsSavingEntityId != null || _lightPurposeBulkSaving) ...[
                const SizedBox(width: 10),
                const SizedBox(
                  width: 16,
                  height: 16,
                  child: CircularProgressIndicator(strokeWidth: 2),
                ),
              ],
            ],
          ),
          const SizedBox(height: 10),
          _SectionManagementRows(
            sections: _sectionOptions,
            savingKey: _sectionsSavingKey,
            enabled: _modernExpertCanEditSections,
            onCreate: _createSection,
            onRename: _renameSection,
            onDelete: _deleteSection,
          ),
          const SizedBox(height: 10),
          if (_lightRows.isEmpty)
            Text(
              'No lights found for this area.',
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.85),
                fontSize: 12,
              ),
            )
          else
            for (final group in _groupLightsBySection(_lightRows)) ...[
              _LightGroupHeader(
                name: group.name,
                count: group.rows.length,
              ),
              for (final light in group.rows)
                _LightPurposeRow(
                  light: light,
                  sections: _sectionOptions,
                  options: _optionsWithCurrent(
                    light.purpose,
                    _lightPresetOptions,
                  ),
                  dirty:
                      _pendingLightPurposeChanges.containsKey(light.entityId),
                  saving: _lightPurposeBulkSaving ||
                      _lightsSavingEntityId == light.entityId,
                  identifying: _identifyingLightEntityId == light.entityId,
                  sectionEnabled: _modernExpertCanAssignLightSections,
                  purposeEnabled: _modernExpertCanEditLightPurposes,
                  onSectionChanged: (sectionId) => unawaited(
                    _assignLightSection(light, sectionId),
                  ),
                  onChanged: (purpose) => _stageLightPurpose(light, purpose),
                  onIdentify: () => unawaited(_identifyLight(light)),
                ),
            ],
        ],
      ),
    );
  }

  Widget _buildControlsPanel() {
    final sectionIds = _areaSectionIds;
    return _ExpertPanel(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              const Expanded(
                child: Text(
                  'Controls',
                  style: TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 16,
                    fontWeight: FontWeight.w700,
                  ),
                ),
              ),
              Text(
                _areaControls.isEmpty ? '' : '${_areaControls.length}',
                style: const TextStyle(
                  color: CelestialColors.textSecondary,
                  fontSize: 13,
                  fontWeight: FontWeight.w800,
                  fontFeatures: [FontFeature.tabularFigures()],
                ),
              ),
            ],
          ),
          if (_controlsError != null) ...[
            const SizedBox(height: 8),
            Text(
              'Controls unavailable: $_controlsError',
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: const TextStyle(
                color: _dangerAccent,
                fontSize: 12,
                fontWeight: FontWeight.w600,
              ),
            ),
          ],
          const SizedBox(height: 10),
          if (_areaControls.isEmpty && _controlsError == null)
            Text(
              'No controls reach this area.',
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.85),
                fontSize: 12,
              ),
            )
          else
            for (final control in _areaControls)
              _AreaControlRow(
                control: control,
                areaId: widget.area.id,
                sectionIds: sectionIds,
                pulseWindowHours: _expertControlsPulseWindowHours(
                  _expertConfig,
                ),
                recentWindowMinutes: _expertControlsRecentWindowMinutes(
                  _expertConfig,
                ),
                onOpen: () => _openControl(control),
              ),
        ],
      ),
    );
  }

  Set<String> get _areaSectionIds => {
        for (final section in _sectionOptions) section.id,
      };

  Widget _buildMotionPanel() {
    return _ExpertPanel(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _SettingsPanelHeader(
            title: 'Motion',
            saving: _saving,
          ),
          _OptionRow(
            label: 'Function',
            value: _settings.motionFunction,
            options: _optionsWithCurrent(
              _settings.motionFunction,
              const ['disabled', 'boost', 'on_off', 'on_only'],
            ),
            enabled: _modernExpertCanEditAreaSettings,
            onChanged: (value) => _queueSettingsUpdate({
              'motion_function': value,
            }),
          ),
          _ExpertSliderRow(
            label: 'Duration',
            value:
                _settings.motionDuration.toDouble().clamp(15, 240).toDouble(),
            min: 15,
            max: 240,
            divisions: 15,
            display: _formatMinutes(_settings.motionDuration),
            enabled: _modernExpertCanEditAreaSettings,
            onChanged: (value) => _queueSettingsUpdate({
              'motion_duration': value.round(),
            }),
          ),
        ],
      ),
    );
  }

  Widget _buildSchedulePanel({
    required String title,
    required String prefix,
    required bool enabled,
    required String source,
    required int offset,
    required int fade,
    required String? nextValue,
    required List<String> sourceOptions,
  }) {
    final isAutoOn = prefix == 'auto_on';
    final primaryDays = isAutoOn ? _settings.autoOnDays : _settings.autoOffDays;
    final secondaryDays =
        isAutoOn ? _settings.autoOnDays2 : _settings.autoOffDays2;
    final time1 = isAutoOn ? _settings.autoOnTime1 : _settings.autoOffTime1;
    final time2 = isAutoOn ? _settings.autoOnTime2 : _settings.autoOffTime2;
    final override =
        isAutoOn ? _settings.autoOnOverride : _settings.autoOffOverride;
    const editable = _modernExpertCanEditAreaSettings;

    return _ExpertPanel(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Expanded(
                child: _SettingsPanelHeader(
                  title: title,
                  saving: _saving,
                ),
              ),
              Switch(
                value: enabled,
                activeThumbColor: _expertAccent,
                onChanged: editable
                    ? (value) => _queueSettingsUpdate({
                          '${prefix}_enabled': value,
                        })
                    : null,
              ),
            ],
          ),
          if (nextValue != null)
            Padding(
              padding: const EdgeInsets.only(bottom: 4),
              child: Text(
                'Next ${_shortServerTime(nextValue)}',
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: const TextStyle(
                  color: CelestialColors.textSecondary,
                  fontSize: 12,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
          if (enabled)
            _ScheduleOverrideRow(
              scheduleOverride: override,
              enabled: editable,
              onSet: () => unawaited(
                _openAutoScheduleOverride(
                  prefix: prefix,
                  title: title,
                  seedTime: time1,
                ),
              ),
              onClear: override == null
                  ? null
                  : () => _clearAutoScheduleOverride(prefix),
            ),
          _OptionRow(
            label: 'Source',
            value: source,
            options: _optionsWithCurrent(source, sourceOptions),
            enabled: editable,
            onChanged: (value) => _queueSettingsUpdate({
              '${prefix}_source': value,
            }),
          ),
          _WeekdayChips(
            label: 'Days',
            selectedDays: primaryDays,
            enabled: editable,
            onToggle: (day) => _updateScheduleDays(prefix, primaryDays, day),
          ),
          if (source == 'custom') ...[
            _TimeSettingRow(
              label: 'Schedule 1',
              value: time1,
              enabled: editable,
              onPick: () => unawaited(
                _pickScheduleTime('${prefix}_time_1', time1),
              ),
              onClear: time1 == null
                  ? null
                  : () => _queueSettingsUpdate({'${prefix}_time_1': null}),
            ),
            if (time2 == null)
              Align(
                alignment: Alignment.centerLeft,
                child: _CompactTextButton(
                  label: '+ add schedule',
                  onPressed: editable
                      ? () => _queueSettingsUpdate({
                            '${prefix}_time_2': isAutoOn ? 9.0 : 23.0,
                            '${prefix}_days_2': const [0, 1, 2, 3, 4, 5, 6],
                          })
                      : null,
                ),
              )
            else ...[
              _WeekdayChips(
                label: 'Days 2',
                selectedDays: secondaryDays,
                enabled: editable,
                onToggle: (day) => _updateScheduleDays(
                  prefix,
                  secondaryDays,
                  day,
                  secondary: true,
                ),
              ),
              _TimeSettingRow(
                label: 'Schedule 2',
                value: time2,
                enabled: editable,
                onPick: () => unawaited(
                  _pickScheduleTime('${prefix}_time_2', time2),
                ),
                onClear: () => _queueSettingsUpdate({
                  '${prefix}_time_2': null,
                  '${prefix}_days_2': const [],
                }),
              ),
            ],
          ] else
            _ExpertSliderRow(
              label: 'Offset',
              value: offset.toDouble().clamp(-180, 180).toDouble(),
              min: -180,
              max: 180,
              divisions: 24,
              display: _formatSignedMinutes(offset),
              enabled: editable,
              onChanged: (value) => _queueSettingsUpdate({
                '${prefix}_offset': _roundToStep(value, 15),
              }),
            ),
          _ExpertSliderRow(
            label: 'Fade',
            value: fade.toDouble().clamp(0, 90).toDouble(),
            min: 0,
            max: 90,
            divisions: 18,
            display: _formatMinutes(fade),
            enabled: editable,
            onChanged: (value) => _queueSettingsUpdate({
              '${prefix}_fade': _roundToStep(value, 5),
            }),
          ),
          if (isAutoOn) ...[
            _OptionRow(
              label: 'Light',
              value: _settings.autoOnLight,
              options: _optionsWithCurrent(
                _settings.autoOnLight,
                const ['circadian', 'nitelite', 'britelite'],
              ),
              enabled: editable,
              onChanged: (value) => _queueSettingsUpdate({
                'auto_on_light': value,
              }),
            ),
            _OptionRow(
              label: 'If',
              value: _settings.autoOnTriggerMode,
              options: _optionsWithCurrent(
                _settings.autoOnTriggerMode,
                const ['always', 'skip_brighter', 'skip_on'],
              ),
              enabled: editable,
              onChanged: (value) => _queueSettingsUpdate({
                'auto_on_trigger_mode': value,
                'auto_on_skip_if_brighter': value == 'skip_brighter',
              }),
            ),
          ] else
            _ScheduleSwitchRow(
              label: 'Only untouched lights',
              value: _settings.autoOffOnlyUntouched,
              enabled: editable,
              onChanged: (value) => _queueSettingsUpdate({
                'auto_off_only_untouched': value,
              }),
            ),
          if (_scheduleParticipants.isNotEmpty)
            _ScheduleParticipationRows(
              participants: _scheduleParticipants,
              autoOn: isAutoOn,
              savingKey: _participationSavingKey,
              enabled: _modernExpertCanEditScheduleParticipation,
              onChanged: (participant, value) => unawaited(
                _saveScheduleParticipation(
                  participant,
                  autoOn: isAutoOn,
                  value: value,
                ),
              ),
            ),
        ],
      ),
    );
  }
}

class _SettingsPanelHeader extends StatelessWidget {
  final String title;
  final bool saving;

  const _SettingsPanelHeader({
    required this.title,
    required this.saving,
  });

  @override
  Widget build(BuildContext context) {
    return Row(
      children: [
        Expanded(
          child: Text(
            title,
            style: const TextStyle(
              color: CelestialColors.textPrimary,
              fontSize: 16,
              fontWeight: FontWeight.w700,
            ),
          ),
        ),
        AnimatedSwitcher(
          duration: const Duration(milliseconds: 150),
          child: saving
              ? const SizedBox(
                  key: ValueKey('saving'),
                  width: 14,
                  height: 14,
                  child: CircularProgressIndicator(strokeWidth: 2),
                )
              : const SizedBox(
                  key: ValueKey('saved'),
                  width: 14,
                  height: 14,
                ),
        ),
      ],
    );
  }
}

class _WeekdayChips extends StatelessWidget {
  static const _labels = ['Mo', 'Tu', 'We', 'Th', 'Fr', 'Sa', 'Su'];

  final String label;
  final List<int> selectedDays;
  final bool enabled;
  final ValueChanged<int> onToggle;

  const _WeekdayChips({
    required this.label,
    required this.selectedDays,
    this.enabled = true,
    required this.onToggle,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(top: 10),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            label,
            style: const TextStyle(
              color: CelestialColors.textSecondary,
              fontSize: 13,
              fontWeight: FontWeight.w600,
            ),
          ),
          const SizedBox(height: 8),
          Row(
            children: [
              for (var day = 0; day < _labels.length; day++) ...[
                Expanded(
                  child: GestureDetector(
                    onTap: enabled ? () => onToggle(day) : null,
                    behavior: HitTestBehavior.opaque,
                    child: AnimatedContainer(
                      duration: const Duration(milliseconds: 120),
                      height: 32,
                      alignment: Alignment.center,
                      decoration: BoxDecoration(
                        color: selectedDays.contains(day)
                            ? _expertAccent.withValues(alpha: 0.16)
                            : _panel2Color,
                        borderRadius: BorderRadius.circular(7),
                        border: Border.all(
                          color: selectedDays.contains(day)
                              ? _expertAccent.withValues(alpha: 0.65)
                              : CelestialColors.orbitRing,
                        ),
                      ),
                      child: Text(
                        _labels[day],
                        style: TextStyle(
                          color: selectedDays.contains(day)
                              ? _expertAccent
                              : CelestialColors.textSecondary,
                          fontSize: 11,
                          fontWeight: FontWeight.w800,
                        ),
                      ),
                    ),
                  ),
                ),
                if (day != _labels.length - 1) const SizedBox(width: 5),
              ],
            ],
          ),
        ],
      ),
    );
  }
}

class _TimeSettingRow extends StatelessWidget {
  final String label;
  final double? value;
  final bool enabled;
  final VoidCallback onPick;
  final VoidCallback? onClear;

  const _TimeSettingRow({
    required this.label,
    required this.value,
    this.enabled = true,
    required this.onPick,
    required this.onClear,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(top: 8),
      child: Row(
        children: [
          Expanded(
            child: Text(
              label,
              style: const TextStyle(
                color: CelestialColors.textSecondary,
                fontSize: 13,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
          TextButton.icon(
            onPressed: enabled ? onPick : null,
            icon: const Icon(Icons.schedule_rounded, size: 16),
            label: Text(value == null ? 'Pick time' : _formatHour(value!)),
            style: TextButton.styleFrom(
              foregroundColor: CelestialColors.textPrimary,
              padding: const EdgeInsets.symmetric(horizontal: 10),
              minimumSize: const Size(0, 34),
              shape: RoundedRectangleBorder(
                borderRadius: BorderRadius.circular(7),
              ),
              side: const BorderSide(color: CelestialColors.orbitRing),
            ),
          ),
          const SizedBox(width: 6),
          IconButton(
            onPressed: enabled ? onClear : null,
            tooltip: 'Clear time',
            icon: const Icon(Icons.close_rounded),
            iconSize: 16,
            style: IconButton.styleFrom(
              foregroundColor: onClear == null
                  ? CelestialColors.textSecondary.withValues(alpha: 0.28)
                  : _dangerAccent,
              fixedSize: const Size(32, 32),
              minimumSize: const Size(32, 32),
              padding: EdgeInsets.zero,
              shape: RoundedRectangleBorder(
                borderRadius: BorderRadius.circular(7),
              ),
              side: BorderSide(
                color: onClear == null
                    ? CelestialColors.orbitRing.withValues(alpha: 0.5)
                    : _dangerAccent.withValues(alpha: 0.45),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _ScheduleSwitchRow extends StatelessWidget {
  final String label;
  final bool value;
  final bool enabled;
  final ValueChanged<bool> onChanged;

  const _ScheduleSwitchRow({
    required this.label,
    required this.value,
    this.enabled = true,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(top: 8),
      child: Row(
        children: [
          Expanded(
            child: Text(
              label,
              style: const TextStyle(
                color: CelestialColors.textSecondary,
                fontSize: 13,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
          Switch(
            value: value,
            activeThumbColor: _expertAccent,
            onChanged: enabled ? onChanged : null,
          ),
        ],
      ),
    );
  }
}

class _ScheduleOverrideRow extends StatelessWidget {
  final _ExpertAutoScheduleOverride? scheduleOverride;
  final bool enabled;
  final VoidCallback onSet;
  final VoidCallback? onClear;

  const _ScheduleOverrideRow({
    required this.scheduleOverride,
    this.enabled = true,
    required this.onSet,
    required this.onClear,
  });

  @override
  Widget build(BuildContext context) {
    final active = scheduleOverride != null;
    return Container(
      margin: const EdgeInsets.only(top: 8, bottom: 4),
      padding: const EdgeInsets.fromLTRB(10, 8, 8, 8),
      decoration: BoxDecoration(
        color: active
            ? _expertAccent.withValues(alpha: 0.09)
            : _panel2Color.withValues(alpha: 0.52),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(
          color: active
              ? _expertAccent.withValues(alpha: 0.5)
              : CelestialColors.orbitRing,
        ),
      ),
      child: Row(
        children: [
          Icon(
            active ? Icons.event_busy_rounded : Icons.event_repeat_rounded,
            size: 18,
            color: active ? _expertAccent : CelestialColors.textSecondary,
          ),
          const SizedBox(width: 8),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  active ? 'Override active' : 'Normal schedule',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 13,
                    fontWeight: FontWeight.w800,
                  ),
                ),
                Text(
                  scheduleOverride?.label ?? 'Set a temporary schedule change',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(
                    color: CelestialColors.textSecondary,
                    fontSize: 10,
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(width: 8),
          _CompactTextButton(
            label: active ? 'Change' : 'Set',
            onPressed: enabled ? onSet : null,
          ),
          if (active) ...[
            const SizedBox(width: 6),
            _SmallSquareButton(
              icon: Icons.close_rounded,
              onPressed: enabled ? onClear : null,
            ),
          ],
        ],
      ),
    );
  }
}

class _AutoScheduleOverrideSheet extends StatefulWidget {
  final String title;
  final double initialTime;

  const _AutoScheduleOverrideSheet({
    required this.title,
    required this.initialTime,
  });

  @override
  State<_AutoScheduleOverrideSheet> createState() =>
      _AutoScheduleOverrideSheetState();
}

class _AutoScheduleOverrideSheetState
    extends State<_AutoScheduleOverrideSheet> {
  bool _pause = true;
  String _through = 'tomorrow';
  late double _time = _roundQuarter(widget.initialTime);

  @override
  Widget build(BuildContext context) {
    return SafeArea(
      child: Container(
        margin: const EdgeInsets.all(12),
        padding: EdgeInsets.fromLTRB(
          16,
          14,
          16,
          16 + MediaQuery.viewInsetsOf(context).bottom,
        ),
        decoration: BoxDecoration(
          color: CelestialColors.backgroundCard,
          borderRadius: BorderRadius.circular(8),
          border: Border.all(color: CelestialColors.orbitRing),
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(
              children: [
                Expanded(
                  child: Text(
                    '${widget.title} Override',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 17,
                      fontWeight: FontWeight.w800,
                    ),
                  ),
                ),
                IconButton(
                  onPressed: () => Navigator.of(context).pop(),
                  tooltip: 'Close',
                  icon: const Icon(Icons.close_rounded),
                  color: CelestialColors.textSecondary,
                ),
              ],
            ),
            const SizedBox(height: 8),
            _OverrideChoiceGroup(
              label: 'Mode',
              value: _pause ? 'pause' : 'custom',
              options: const {'pause': 'Pause', 'custom': 'Custom time'},
              onChanged: (value) => setState(() => _pause = value == 'pause'),
            ),
            const SizedBox(height: 10),
            _OverrideChoiceGroup(
              label: 'Through',
              value: _through,
              options: const {
                'today': 'Today',
                'tomorrow': 'Tomorrow',
                'forever': 'Forever',
              },
              onChanged: (value) => setState(() => _through = value),
            ),
            if (!_pause) ...[
              const SizedBox(height: 10),
              Row(
                children: [
                  const Expanded(
                    child: Text(
                      'Time',
                      style: TextStyle(
                        color: CelestialColors.textSecondary,
                        fontSize: 13,
                        fontWeight: FontWeight.w700,
                      ),
                    ),
                  ),
                  Text(
                    _formatHour(_time),
                    style: const TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 13,
                      fontWeight: FontWeight.w800,
                      fontFeatures: [FontFeature.tabularFigures()],
                    ),
                  ),
                ],
              ),
              SliderTheme(
                data: SliderTheme.of(context).copyWith(
                  activeTrackColor: _expertAccent,
                  inactiveTrackColor: CelestialColors.orbitRing,
                  thumbColor: _expertAccent,
                  overlayColor: _expertAccent.withValues(alpha: 0.14),
                  trackHeight: 3,
                ),
                child: Slider(
                  value: _time.clamp(0, 23.75),
                  min: 0,
                  max: 23.75,
                  divisions: 95,
                  onChanged: (value) =>
                      setState(() => _time = _roundQuarter(value)),
                ),
              ),
            ],
            const SizedBox(height: 12),
            FilledButton.icon(
              onPressed: () {
                final untilDate = _scheduleOverrideUntilDate(_through);
                Navigator.of(context).pop(
                  _pause
                      ? {
                          'mode': 'pause',
                          'until_date': untilDate,
                        }
                      : {
                          'mode': _through,
                          'until_date': untilDate,
                          'time': _roundQuarter(_time),
                        },
                );
              },
              icon: const Icon(Icons.check_rounded),
              label: const Text('Apply'),
              style: FilledButton.styleFrom(
                backgroundColor: _expertAccent,
                foregroundColor: const Color(0xFF1A120C),
                shape: RoundedRectangleBorder(
                  borderRadius: BorderRadius.circular(8),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _OverrideChoiceGroup extends StatelessWidget {
  final String label;
  final String value;
  final Map<String, String> options;
  final ValueChanged<String> onChanged;

  const _OverrideChoiceGroup({
    required this.label,
    required this.value,
    required this.options,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          label,
          style: const TextStyle(
            color: CelestialColors.textSecondary,
            fontSize: 12,
            fontWeight: FontWeight.w700,
          ),
        ),
        const SizedBox(height: 6),
        Wrap(
          spacing: 6,
          runSpacing: 6,
          children: [
            for (final entry in options.entries)
              ChoiceChip(
                label: Text(entry.value),
                selected: value == entry.key,
                showCheckmark: false,
                selectedColor: _expertAccent.withValues(alpha: 0.18),
                backgroundColor: _panel2Color,
                side: BorderSide(
                  color: value == entry.key
                      ? _expertAccent.withValues(alpha: 0.55)
                      : CelestialColors.orbitRing,
                ),
                labelStyle: TextStyle(
                  color: value == entry.key
                      ? _expertAccent
                      : CelestialColors.textSecondary,
                  fontSize: 12,
                  fontWeight: FontWeight.w800,
                ),
                onSelected: (_) => onChanged(entry.key),
              ),
          ],
        ),
      ],
    );
  }
}

class _ScheduleParticipationRows extends StatelessWidget {
  final List<_ExpertScheduleParticipant> participants;
  final bool autoOn;
  final String? savingKey;
  final bool enabled;
  final void Function(_ExpertScheduleParticipant participant, bool value)
      onChanged;

  const _ScheduleParticipationRows({
    required this.participants,
    required this.autoOn,
    required this.savingKey,
    this.enabled = true,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(top: 10),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          const Text(
            'Sections',
            style: TextStyle(
              color: CelestialColors.textSecondary,
              fontSize: 13,
              fontWeight: FontWeight.w600,
            ),
          ),
          const SizedBox(height: 6),
          for (final participant in participants)
            _ScheduleParticipationRow(
              participant: participant,
              autoOn: autoOn,
              saving: savingKey == '${participant.id}:${autoOn ? 'on' : 'off'}',
              enabled: enabled,
              onChanged: (value) => onChanged(participant, value),
            ),
        ],
      ),
    );
  }
}

class _ScheduleParticipationRow extends StatelessWidget {
  final _ExpertScheduleParticipant participant;
  final bool autoOn;
  final bool saving;
  final bool enabled;
  final ValueChanged<bool> onChanged;

  const _ScheduleParticipationRow({
    required this.participant,
    required this.autoOn,
    required this.saving,
    this.enabled = true,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    final enabled = autoOn
        ? participant.participatesInAutoOn
        : participant.participatesInAutoOff;
    return Container(
      margin: const EdgeInsets.only(top: 6),
      padding: const EdgeInsets.fromLTRB(10, 6, 8, 6),
      decoration: BoxDecoration(
        color: _panel2Color.withValues(alpha: 0.48),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: CelestialColors.orbitRing),
      ),
      child: Row(
        children: [
          Expanded(
            child: Text(
              participant.name,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: const TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 13,
                fontWeight: FontWeight.w700,
              ),
            ),
          ),
          if (saving)
            const SizedBox(
              width: 18,
              height: 18,
              child: CircularProgressIndicator(strokeWidth: 2),
            )
          else
            Switch(
              value: enabled,
              activeThumbColor: _expertAccent,
              onChanged: saving || !this.enabled ? null : onChanged,
            ),
        ],
      ),
    );
  }
}

class _AreaActionTile extends StatelessWidget {
  final IconData icon;
  final String label;
  final String value;
  final bool active;
  final bool busy;
  final VoidCallback onTap;

  const _AreaActionTile({
    required this.icon,
    required this.label,
    required this.value,
    required this.active,
    required this.busy,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    final color = active ? _expertAccent : CelestialColors.textSecondary;
    return GestureDetector(
      onTap: busy ? null : onTap,
      behavior: HitTestBehavior.opaque,
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 140),
        height: 82,
        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 10),
        decoration: BoxDecoration(
          color: active
              ? _expertAccent.withValues(alpha: 0.12)
              : _panel2Color.withValues(alpha: 0.72),
          borderRadius: BorderRadius.circular(8),
          border: Border.all(
            color: active
                ? _expertAccent.withValues(alpha: 0.55)
                : CelestialColors.orbitRing,
          ),
        ),
        child: Column(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Icon(icon, color: color, size: 22),
            const SizedBox(height: 6),
            Text(
              label,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                color: color,
                fontSize: 11,
                fontWeight: FontWeight.w800,
              ),
            ),
            const SizedBox(height: 2),
            Text(
              value,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                color: color.withValues(alpha: active ? 0.8 : 0.6),
                fontSize: 10,
                fontWeight: FontWeight.w600,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _ZoneSyncButton extends StatelessWidget {
  final IconData icon;
  final String label;
  final bool active;
  final bool enabled;
  final bool highlighted;
  final bool danger;
  final VoidCallback onPressed;

  const _ZoneSyncButton({
    required this.icon,
    required this.label,
    required this.active,
    required this.enabled,
    required this.onPressed,
    this.highlighted = false,
    this.danger = false,
  });

  @override
  Widget build(BuildContext context) {
    final accent = danger
        ? _dangerAccent
        : highlighted
            ? _changedAccent
            : _expertAccent;
    final color = active || highlighted || danger
        ? accent
        : CelestialColors.textSecondary;
    return OutlinedButton(
      onPressed: enabled ? onPressed : null,
      style: OutlinedButton.styleFrom(
        foregroundColor: color,
        side: BorderSide(
          color: active || highlighted || danger
              ? accent.withValues(alpha: 0.75)
              : CelestialColors.orbitRing,
        ),
        backgroundColor: active || highlighted
            ? accent.withValues(alpha: 0.10)
            : _panel2Color.withValues(alpha: 0.50),
        padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 10),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(icon, size: 18),
          const SizedBox(height: 5),
          Text(
            label,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            textAlign: TextAlign.center,
            style: const TextStyle(
              fontSize: 11,
              fontWeight: FontWeight.w800,
            ),
          ),
        ],
      ),
    );
  }
}

class _AreaMetricChip extends StatelessWidget {
  final IconData icon;
  final String label;
  final String value;

  const _AreaMetricChip({
    required this.icon,
    required this.label,
    required this.value,
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 7),
      decoration: BoxDecoration(
        color: _panel2Color.withValues(alpha: 0.68),
        borderRadius: BorderRadius.circular(7),
        border: Border.all(color: CelestialColors.orbitRing),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(icon, color: _expertAccent, size: 14),
          const SizedBox(width: 6),
          Text(
            label,
            style: const TextStyle(
              color: CelestialColors.textSecondary,
              fontSize: 11,
              fontWeight: FontWeight.w700,
            ),
          ),
          const SizedBox(width: 6),
          Text(
            value,
            style: const TextStyle(
              color: CelestialColors.textPrimary,
              fontSize: 12,
              fontWeight: FontWeight.w800,
              fontFeatures: [FontFeature.tabularFigures()],
            ),
          ),
        ],
      ),
    );
  }
}

class _SectionAdjustRows extends StatelessWidget {
  final List<_ExpertSectionAdjustRow> rows;
  final double areaBrightness;
  final List<String> durationPresetValues;
  final int defaultAutoOffMinutes;
  final String? savingKey;
  final bool canClearBrightnessOverride;
  final ValueChanged<_ExpertSectionAdjustRow> onPower;
  final void Function(_ExpertSectionAdjustRow row, double? value) onBrightness;
  final void Function(_ExpertSectionAdjustRow row, String direction) onStep;
  final void Function(_ExpertSectionAdjustRow row, int? durationMinutes)
      onAutoOff;

  const _SectionAdjustRows({
    required this.rows,
    required this.areaBrightness,
    required this.durationPresetValues,
    required this.defaultAutoOffMinutes,
    required this.savingKey,
    this.canClearBrightnessOverride = true,
    required this.onPower,
    required this.onBrightness,
    required this.onStep,
    required this.onAutoOff,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(top: 8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          const Padding(
            padding: EdgeInsets.only(left: 2, bottom: 2),
            child: Text(
              'Sections',
              style: TextStyle(
                color: CelestialColors.textSecondary,
                fontSize: 12,
                fontWeight: FontWeight.w800,
              ),
            ),
          ),
          for (final row in rows)
            _SectionAdjustRow(
              row: row,
              areaBrightness: areaBrightness,
              durationPresetValues: durationPresetValues,
              defaultAutoOffMinutes: defaultAutoOffMinutes,
              savingKey: savingKey,
              canClearBrightnessOverride: canClearBrightnessOverride,
              onPower: () => onPower(row),
              onBrightness: (value) => onBrightness(row, value),
              onStep: (direction) => onStep(row, direction),
              onAutoOff: (durationMinutes) => onAutoOff(row, durationMinutes),
            ),
        ],
      ),
    );
  }
}

class _SectionAdjustRow extends StatefulWidget {
  final _ExpertSectionAdjustRow row;
  final double areaBrightness;
  final List<String> durationPresetValues;
  final int defaultAutoOffMinutes;
  final String? savingKey;
  final bool canClearBrightnessOverride;
  final VoidCallback onPower;
  final ValueChanged<double?> onBrightness;
  final ValueChanged<String> onStep;
  final ValueChanged<int?> onAutoOff;

  const _SectionAdjustRow({
    required this.row,
    required this.areaBrightness,
    required this.durationPresetValues,
    required this.defaultAutoOffMinutes,
    required this.savingKey,
    this.canClearBrightnessOverride = true,
    required this.onPower,
    required this.onBrightness,
    required this.onStep,
    required this.onAutoOff,
  });

  @override
  State<_SectionAdjustRow> createState() => _SectionAdjustRowState();
}

class _SectionAdjustRowState extends State<_SectionAdjustRow> {
  double? _draftBrightness;

  @override
  void didUpdateWidget(covariant _SectionAdjustRow oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.row.id != widget.row.id ||
        oldWidget.row.currentBrightness != widget.row.currentBrightness ||
        oldWidget.areaBrightness != widget.areaBrightness) {
      _draftBrightness = null;
    }
  }

  @override
  Widget build(BuildContext context) {
    final row = widget.row;
    final saving = widget.savingKey?.startsWith('${row.id}:') == true;
    final inheritedBrightness = row.homeBrightness ?? widget.areaBrightness;
    final brightness = (_draftBrightness ??
            row.currentBrightness ??
            (row.brightnessOverride == null
                ? inheritedBrightness
                : inheritedBrightness + row.brightnessOverride!))
        .clamp(0, 100)
        .toDouble();
    final accent = row.hasBrightnessOverride ? _changedAccent : _expertAccent;

    return Container(
      margin: const EdgeInsets.only(top: 7),
      padding: const EdgeInsets.fromLTRB(8, 8, 8, 6),
      decoration: BoxDecoration(
        color: _panel2Color.withValues(alpha: 0.42),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: CelestialColors.orbitRing),
      ),
      child: Column(
        children: [
          Row(
            children: [
              IconButton(
                onPressed: saving ? null : widget.onPower,
                tooltip: 'Power',
                icon: saving && widget.savingKey == '${row.id}:power'
                    ? const SizedBox(
                        width: 16,
                        height: 16,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      )
                    : Icon(
                        Icons.power_settings_new_rounded,
                        color: row.isOn
                            ? _expertAccent
                            : CelestialColors.textSecondary,
                      ),
                iconSize: 18,
                style: IconButton.styleFrom(
                  backgroundColor: row.isOn
                      ? _expertAccent.withValues(alpha: 0.12)
                      : _panelColor,
                  fixedSize: const Size(32, 32),
                  minimumSize: const Size(32, 32),
                  padding: EdgeInsets.zero,
                  shape: RoundedRectangleBorder(
                    borderRadius: BorderRadius.circular(7),
                  ),
                  side: BorderSide(
                    color: row.isOn ? _expertAccent : CelestialColors.orbitRing,
                  ),
                ),
              ),
              const SizedBox(width: 8),
              Expanded(
                child: Text(
                  row.name,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 13,
                    fontWeight: FontWeight.w800,
                  ),
                ),
              ),
              _SectionAutoOffButton(
                autoOffAt: row.autoOffAt,
                durationPresetValues: widget.durationPresetValues,
                defaultAutoOffMinutes: widget.defaultAutoOffMinutes,
                saving: widget.savingKey == '${row.id}:auto_off',
                disabled: saving,
                onSelected: widget.onAutoOff,
              ),
              const SizedBox(width: 6),
              _SliderStepButton(
                icon: Icons.remove_rounded,
                onPressed: saving ? null : () => widget.onStep('down'),
              ),
              const SizedBox(width: 4),
              _SliderStepButton(
                icon: Icons.add_rounded,
                onPressed: saving ? null : () => widget.onStep('up'),
              ),
              const SizedBox(width: 8),
              GestureDetector(
                onTap: row.hasBrightnessOverride &&
                        !saving &&
                        widget.canClearBrightnessOverride
                    ? () => widget.onBrightness(null)
                    : null,
                behavior: HitTestBehavior.opaque,
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(
                      '${brightness.round()}%',
                      style: TextStyle(
                        color: row.hasBrightnessOverride
                            ? _changedAccent
                            : CelestialColors.textPrimary,
                        fontSize: 13,
                        fontWeight: FontWeight.w800,
                        fontFeatures: const [FontFeature.tabularFigures()],
                      ),
                    ),
                    const SizedBox(width: 4),
                    Icon(
                      Icons.restart_alt_rounded,
                      color: row.hasBrightnessOverride &&
                              !saving &&
                              widget.canClearBrightnessOverride
                          ? _changedAccent
                          : CelestialColors.textSecondary
                              .withValues(alpha: 0.28),
                      size: 16,
                    ),
                  ],
                ),
              ),
            ],
          ),
          SliderTheme(
            data: SliderTheme.of(context).copyWith(
              activeTrackColor: accent,
              inactiveTrackColor: CelestialColors.orbitRing,
              thumbColor: accent,
              overlayColor: accent.withValues(alpha: 0.14),
              trackHeight: 4,
            ),
            child: Slider(
              value: brightness,
              min: 0,
              max: 100,
              divisions: 100,
              onChanged: saving
                  ? null
                  : (value) => setState(() => _draftBrightness = value),
              onChangeEnd: saving
                  ? null
                  : (value) {
                      _draftBrightness = null;
                      widget.onBrightness(value.roundToDouble());
                    },
            ),
          ),
        ],
      ),
    );
  }
}

class _SectionAutoOffButton extends StatelessWidget {
  final String? autoOffAt;
  final List<String> durationPresetValues;
  final int defaultAutoOffMinutes;
  final bool saving;
  final bool disabled;
  final ValueChanged<int?> onSelected;

  const _SectionAutoOffButton({
    required this.autoOffAt,
    required this.durationPresetValues,
    required this.defaultAutoOffMinutes,
    required this.saving,
    required this.disabled,
    required this.onSelected,
  });

  @override
  Widget build(BuildContext context) {
    final hasTimer = autoOffAt != null && autoOffAt != 'forever';
    final label = _sectionAutoOffLabel(autoOffAt);
    final color = hasTimer ? _changedAccent : CelestialColors.textSecondary;
    final options = _positiveDurationPresetValues(
      durationPresetValues,
      includeValues: [defaultAutoOffMinutes.toString()],
    );

    if (saving) {
      return const SizedBox(
        width: 32,
        height: 32,
        child: Center(
          child: SizedBox(
            width: 16,
            height: 16,
            child: CircularProgressIndicator(strokeWidth: 2),
          ),
        ),
      );
    }

    return PopupMenuButton<String>(
      enabled: !disabled,
      tooltip: 'Section auto-off',
      color: _panel2Color,
      onSelected: (value) {
        if (value == 'clear') {
          onSelected(null);
        } else {
          onSelected(int.parse(value));
        }
      },
      itemBuilder: (context) => [
        if (hasTimer)
          const PopupMenuItem(
            value: 'clear',
            child: Text('Clear timer'),
          ),
        for (final value in options)
          PopupMenuItem(
            value: value,
            child: Text(
              value == defaultAutoOffMinutes.toString()
                  ? 'Default ${_durationOptionLabel(value)}'
                  : _durationOptionLabel(value),
            ),
          ),
      ],
      child: Container(
        height: 32,
        constraints: const BoxConstraints(minWidth: 38),
        padding: const EdgeInsets.symmetric(horizontal: 7),
        decoration: BoxDecoration(
          color:
              hasTimer ? _changedAccent.withValues(alpha: 0.12) : _panelColor,
          borderRadius: BorderRadius.circular(7),
          border: Border.all(
            color: hasTimer ? _changedAccent : CelestialColors.orbitRing,
          ),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.timer_outlined, color: color, size: 15),
            if (label != null) ...[
              const SizedBox(width: 4),
              Text(
                label,
                style: TextStyle(
                  color: color,
                  fontSize: 11,
                  fontWeight: FontWeight.w800,
                  fontFeatures: const [FontFeature.tabularFigures()],
                ),
              ),
            ],
          ],
        ),
      ),
    );
  }
}

class _AdjustSliderRow extends StatelessWidget {
  final String label;
  final double value;
  final double min;
  final double max;
  final int divisions;
  final String display;
  final bool dirty;
  final bool sliderEnabled;
  final ValueChanged<double> onChanged;
  final ValueChanged<double> onChangeEnd;
  final VoidCallback onDecrement;
  final VoidCallback onIncrement;
  final VoidCallback? onReset;
  final List<_SliderPreviewPoint> previewPoints;

  const _AdjustSliderRow({
    required this.label,
    required this.value,
    required this.min,
    required this.max,
    required this.divisions,
    required this.display,
    required this.dirty,
    this.sliderEnabled = true,
    required this.onChanged,
    required this.onChangeEnd,
    required this.onDecrement,
    required this.onIncrement,
    required this.onReset,
    this.previewPoints = const [],
  });

  @override
  Widget build(BuildContext context) {
    final accent = dirty ? _changedAccent : _expertAccent;
    return Padding(
      padding: const EdgeInsets.only(top: 12),
      child: Column(
        children: [
          Row(
            children: [
              Expanded(
                child: Text(
                  label,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 13,
                    fontWeight: FontWeight.w700,
                  ),
                ),
              ),
              _SliderStepButton(
                icon: Icons.remove_rounded,
                onPressed: onDecrement,
              ),
              const SizedBox(width: 4),
              _SliderStepButton(
                icon: Icons.add_rounded,
                onPressed: onIncrement,
              ),
              const SizedBox(width: 8),
              GestureDetector(
                onTap: onReset,
                behavior: HitTestBehavior.opaque,
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(
                      display,
                      style: TextStyle(
                        color: dirty
                            ? _changedAccent
                            : CelestialColors.textPrimary,
                        fontSize: 13,
                        fontWeight: FontWeight.w800,
                        fontFeatures: const [FontFeature.tabularFigures()],
                      ),
                    ),
                    const SizedBox(width: 4),
                    Icon(
                      Icons.restart_alt_rounded,
                      color: onReset == null
                          ? CelestialColors.textSecondary
                              .withValues(alpha: 0.28)
                          : _changedAccent,
                      size: 16,
                    ),
                  ],
                ),
              ),
            ],
          ),
          SliderTheme(
            data: SliderTheme.of(context).copyWith(
              activeTrackColor: accent,
              inactiveTrackColor: CelestialColors.orbitRing,
              thumbColor: accent,
              overlayColor: accent.withValues(alpha: 0.14),
              trackHeight: 5,
            ),
            child: Slider(
              value: value.clamp(min, max).toDouble(),
              min: min,
              max: max,
              divisions: divisions,
              onChanged: sliderEnabled ? onChanged : null,
              onChangeEnd: sliderEnabled ? onChangeEnd : null,
            ),
          ),
          if (previewPoints.length >= 2)
            _SliderPreviewStrip(points: previewPoints),
        ],
      ),
    );
  }
}

class _SliderPreviewStrip extends StatelessWidget {
  final List<_SliderPreviewPoint> points;

  const _SliderPreviewStrip({required this.points});

  @override
  Widget build(BuildContext context) {
    final sorted = List<_SliderPreviewPoint>.from(points)
      ..sort((a, b) => a.brightness.compareTo(b.brightness));
    if (sorted.length < 2) return const SizedBox.shrink();
    return Padding(
      padding: const EdgeInsets.fromLTRB(24, 0, 24, 2),
      child: Container(
        height: 5,
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(999),
          border: Border.all(
            color: CelestialColors.orbitRing.withValues(alpha: 0.55),
            width: 0.5,
          ),
          gradient: LinearGradient(
            colors: [
              for (final point in sorted) _sliderPreviewColor(point),
            ],
          ),
        ),
      ),
    );
  }
}

class _SliderStepButton extends StatelessWidget {
  final IconData icon;
  final VoidCallback? onPressed;

  const _SliderStepButton({
    required this.icon,
    required this.onPressed,
  });

  @override
  Widget build(BuildContext context) {
    return IconButton(
      onPressed: onPressed,
      icon: Icon(icon),
      iconSize: 16,
      style: IconButton.styleFrom(
        foregroundColor: CelestialColors.textPrimary,
        backgroundColor: _panel2Color,
        fixedSize: const Size(28, 28),
        minimumSize: const Size(28, 28),
        padding: EdgeInsets.zero,
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(6)),
        side: const BorderSide(color: CelestialColors.orbitRing),
      ),
    );
  }
}

class _TuneStepSlider extends StatelessWidget {
  final String label;
  final int value;
  final List<String> steps;
  final String display;
  final bool saving;
  final bool enabled;
  final ValueChanged<int> onChanged;
  final ValueChanged<int> onChangeEnd;

  const _TuneStepSlider({
    required this.label,
    required this.value,
    required this.steps,
    required this.display,
    required this.saving,
    this.enabled = true,
    required this.onChanged,
    required this.onChangeEnd,
  });

  @override
  Widget build(BuildContext context) {
    final clamped = value.clamp(0, steps.length - 1).toInt();
    return Padding(
      padding: const EdgeInsets.only(top: 12),
      child: Column(
        children: [
          Row(
            children: [
              Expanded(
                child: Text(
                  label,
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 13,
                    fontWeight: FontWeight.w700,
                  ),
                ),
              ),
              Text(
                display,
                style: const TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 13,
                  fontWeight: FontWeight.w800,
                  fontFeatures: [FontFeature.tabularFigures()],
                ),
              ),
            ],
          ),
          SliderTheme(
            data: SliderTheme.of(context).copyWith(
              activeTrackColor: _expertAccent,
              inactiveTrackColor: CelestialColors.orbitRing,
              thumbColor: _expertAccent,
              overlayColor: _expertAccent.withValues(alpha: 0.14),
              trackHeight: 5,
            ),
            child: Slider(
              value: clamped.toDouble(),
              min: 0,
              max: (steps.length - 1).toDouble(),
              divisions: steps.length - 1,
              label: steps[clamped],
              onChanged: saving || !enabled
                  ? null
                  : (value) => onChanged(value.round()),
              onChangeEnd: saving || !enabled
                  ? null
                  : (value) => onChangeEnd(value.round()),
            ),
          ),
        ],
      ),
    );
  }
}

class _FeedbackTargetRow extends StatelessWidget {
  final String value;
  final List<_ExpertFeedbackTargetChoice> choices;
  final bool saving;
  final bool enabled;
  final ValueChanged<String> onChanged;

  const _FeedbackTargetRow({
    required this.value,
    required this.choices,
    required this.saving,
    this.enabled = true,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    final selected = choices.firstWhere(
      (choice) => choice.value == value,
      orElse: () => choices.first,
    );
    final effectiveValue = choices.any((choice) => choice.value == value)
        ? value
        : choices.first.value;
    return Container(
      margin: const EdgeInsets.only(top: 10),
      padding: const EdgeInsets.fromLTRB(10, 8, 8, 8),
      decoration: BoxDecoration(
        color: _panel2Color.withValues(alpha: 0.58),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: CelestialColors.orbitRing),
      ),
      child: Row(
        children: [
          const Icon(
            Icons.online_prediction_rounded,
            size: 18,
            color: _expertAccent,
          ),
          const SizedBox(width: 8),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                const Text(
                  'Feedback cue',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 13,
                    fontWeight: FontWeight.w800,
                  ),
                ),
                Text(
                  selected.detail,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(
                    color: CelestialColors.textSecondary,
                    fontSize: 10,
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(width: 10),
          if (saving)
            const SizedBox(
              width: 18,
              height: 18,
              child: CircularProgressIndicator(strokeWidth: 2),
            )
          else
            _CompactDropdown<String>(
              value: effectiveValue,
              items: choices.map((choice) => choice.value).toList(),
              itemLabel: (target) =>
                  choices.firstWhere((choice) => choice.value == target).label,
              enabled: enabled,
              onChanged: onChanged,
            ),
        ],
      ),
    );
  }
}

class _SectionTuneRows extends StatelessWidget {
  final List<_ExpertSectionTuneRow> rows;
  final int areaLumenStep;
  final int areaSolarStep;
  final String? savingKey;
  final bool enabled;
  final void Function(
    _ExpertSectionTuneRow row,
    String dimension,
    double? value,
  ) onChanged;

  const _SectionTuneRows({
    required this.rows,
    required this.areaLumenStep,
    required this.areaSolarStep,
    required this.savingKey,
    this.enabled = true,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(top: 14),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          const Text(
            'Sections',
            style: TextStyle(
              color: CelestialColors.textSecondary,
              fontSize: 13,
              fontWeight: FontWeight.w800,
            ),
          ),
          const SizedBox(height: 6),
          for (final row in rows)
            _SectionTuneRow(
              row: row,
              areaLumenStep: areaLumenStep,
              areaSolarStep: areaSolarStep,
              savingKey: savingKey,
              enabled: enabled,
              onChanged: onChanged,
            ),
        ],
      ),
    );
  }
}

class _SectionTuneRow extends StatelessWidget {
  final _ExpertSectionTuneRow row;
  final int areaLumenStep;
  final int areaSolarStep;
  final String? savingKey;
  final bool enabled;
  final void Function(
    _ExpertSectionTuneRow row,
    String dimension,
    double? value,
  ) onChanged;

  const _SectionTuneRow({
    required this.row,
    required this.areaLumenStep,
    required this.areaSolarStep,
    required this.savingKey,
    this.enabled = true,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      margin: const EdgeInsets.only(top: 6),
      padding: const EdgeInsets.all(10),
      decoration: BoxDecoration(
        color: _panel2Color.withValues(alpha: 0.48),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: CelestialColors.orbitRing),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            row.name,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: const TextStyle(
              color: CelestialColors.textPrimary,
              fontSize: 13,
              fontWeight: FontWeight.w800,
            ),
          ),
          const SizedBox(height: 8),
          _SectionTunePicker(
            label: 'Balance',
            dimension: 'balance',
            inheritedStep: areaLumenStep,
            selectedStep:
                row.balance == null ? null : _closestLumenStep(row.balance!),
            steps: _lumenSteps,
            saving: savingKey == '${row.id}:balance',
            enabled: enabled,
            onSelected: (value) => onChanged(row, 'balance', value),
          ),
          const SizedBox(height: 8),
          _SectionTunePicker(
            label: 'Sun dimming',
            dimension: 'sun_dimming',
            inheritedStep: areaSolarStep,
            selectedStep: row.sunDimming == null
                ? null
                : _closestSolarStep(row.sunDimming!),
            steps: _solarSteps,
            saving: savingKey == '${row.id}:sun_dimming',
            enabled: enabled,
            onSelected: (value) => onChanged(row, 'sun_dimming', value),
          ),
        ],
      ),
    );
  }
}

class _SectionTunePicker extends StatelessWidget {
  final String label;
  final String dimension;
  final int inheritedStep;
  final int? selectedStep;
  final List<_TuneStep> steps;
  final bool saving;
  final bool enabled;
  final ValueChanged<double?> onSelected;

  const _SectionTunePicker({
    required this.label,
    required this.dimension,
    required this.inheritedStep,
    required this.selectedStep,
    required this.steps,
    required this.saving,
    this.enabled = true,
    required this.onSelected,
  });

  @override
  Widget build(BuildContext context) {
    final effectiveStep =
        (selectedStep ?? inheritedStep).clamp(0, steps.length - 1).toInt();
    final selectedValue = selectedStep == null ? -1 : effectiveStep;
    final display = _sectionTuneDisplay(
      dimension: dimension,
      step: effectiveStep,
      inherited: selectedStep == null,
    );

    return Row(
      children: [
        Expanded(
          child: Text(
            label,
            style: const TextStyle(
              color: CelestialColors.textSecondary,
              fontSize: 12,
              fontWeight: FontWeight.w700,
            ),
          ),
        ),
        if (saving)
          const SizedBox(
            width: 18,
            height: 18,
            child: CircularProgressIndicator(strokeWidth: 2),
          )
        else
          _CompactDropdown<int>(
            value: selectedValue,
            items: [
              -1,
              for (var index = 0; index < steps.length; index++) index,
            ],
            itemLabel: (value) => value == -1
                ? 'Follow area · $display'
                : _sectionTuneDisplay(
                    dimension: dimension,
                    step: value,
                    inherited: false,
                  ),
            enabled: enabled,
            onChanged: (value) {
              if (value == -1) {
                onSelected(null);
                return;
              }
              final step = steps[value];
              onSelected(
                dimension == 'balance' ? step.factor : step.exposure,
              );
            },
          ),
      ],
    );
  }
}

class _ActivitySortChips extends StatelessWidget {
  final String value;
  final ValueChanged<String> onChanged;

  const _ActivitySortChips({
    required this.value,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    const modes = [
      ('recent', 'Recent'),
      ('area', 'Area'),
      ('source', 'Source'),
      ('action', 'Action'),
    ];
    return Wrap(
      spacing: 8,
      runSpacing: 8,
      children: [
        for (final mode in modes)
          ChoiceChip(
            label: Text(mode.$2),
            selected: value == mode.$1,
            onSelected: (_) => onChanged(mode.$1),
            selectedColor: _expertAccent.withValues(alpha: 0.18),
            backgroundColor: _panel2Color,
            side: BorderSide(
              color:
                  value == mode.$1 ? _expertAccent : CelestialColors.orbitRing,
            ),
            labelStyle: TextStyle(
              color: value == mode.$1
                  ? _expertAccent
                  : CelestialColors.textSecondary,
              fontSize: 12,
              fontWeight: FontWeight.w800,
            ),
          ),
      ],
    );
  }
}

class _ActivityGroupHeader extends StatelessWidget {
  final String label;
  final int count;

  const _ActivityGroupHeader({
    required this.label,
    required this.count,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(top: 10, bottom: 5),
      child: Row(
        children: [
          Expanded(
            child: Text(
              label,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: const TextStyle(
                color: CelestialColors.textSecondary,
                fontSize: 12,
                fontWeight: FontWeight.w900,
              ),
            ),
          ),
          Text(
            '$count',
            style: const TextStyle(
              color: CelestialColors.textSecondary,
              fontSize: 11,
              fontWeight: FontWeight.w700,
              fontFeatures: [FontFeature.tabularFigures()],
            ),
          ),
        ],
      ),
    );
  }
}

class _ActivityEntryRow extends StatelessWidget {
  final _ExpertActivityEntry entry;
  final String? areaName;

  const _ActivityEntryRow({
    required this.entry,
    required this.areaName,
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      margin: const EdgeInsets.only(bottom: 7),
      padding: const EdgeInsets.fromLTRB(10, 9, 10, 9),
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: CelestialColors.orbitRing),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Container(
            width: 9,
            height: 9,
            margin: const EdgeInsets.only(top: 5),
            decoration: BoxDecoration(
              color: entry.isZoneAction ? _changedAccent : _expertAccent,
              shape: BoxShape.circle,
            ),
          ),
          const SizedBox(width: 9),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    Expanded(
                      child: Text(
                        _activityActionLabel(entry.action),
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: const TextStyle(
                          color: CelestialColors.textPrimary,
                          fontSize: 13,
                          fontWeight: FontWeight.w800,
                        ),
                      ),
                    ),
                    Text(
                      _historyTimeLabel(entry.ts),
                      style: const TextStyle(
                        color: CelestialColors.textSecondary,
                        fontSize: 11,
                        fontFeatures: [FontFeature.tabularFigures()],
                      ),
                    ),
                  ],
                ),
                const SizedBox(height: 2),
                Text(
                  _activitySubtitle(entry, areaName),
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(
                    color: CelestialColors.textSecondary,
                    fontSize: 11,
                    height: 1.25,
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

class _MomentSortChips extends StatelessWidget {
  final String value;
  final ValueChanged<String> onChanged;

  const _MomentSortChips({
    required this.value,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    const modes = [
      ('name', 'Name'),
      ('action', 'Action'),
      ('usage', 'Usage'),
    ];
    return Wrap(
      spacing: 8,
      runSpacing: 8,
      children: [
        for (final mode in modes)
          ChoiceChip(
            label: Text(mode.$2),
            selected: value == mode.$1,
            onSelected: (_) => onChanged(mode.$1),
            selectedColor: _expertAccent.withValues(alpha: 0.18),
            backgroundColor: _panel2Color,
            side: BorderSide(
              color:
                  value == mode.$1 ? _expertAccent : CelestialColors.orbitRing,
            ),
            labelStyle: TextStyle(
              color: value == mode.$1
                  ? _expertAccent
                  : CelestialColors.textSecondary,
              fontSize: 12,
              fontWeight: FontWeight.w800,
            ),
          ),
      ],
    );
  }
}

class _MomentRow extends StatelessWidget {
  final _ExpertMoment moment;
  final bool busy;
  final VoidCallback onOpen;
  final VoidCallback onRun;
  final VoidCallback onDelete;

  const _MomentRow({
    required this.moment,
    required this.busy,
    required this.onOpen,
    required this.onRun,
    required this.onDelete,
  });

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: onOpen,
      behavior: HitTestBehavior.opaque,
      child: Container(
        margin: const EdgeInsets.only(bottom: 8),
        padding: const EdgeInsets.fromLTRB(12, 10, 8, 10),
        decoration: BoxDecoration(
          color: CelestialColors.backgroundCard,
          borderRadius: BorderRadius.circular(8),
          border: Border.all(color: CelestialColors.orbitRing),
        ),
        child: Row(
          children: [
            Container(
              width: 34,
              height: 34,
              decoration: BoxDecoration(
                color: _expertAccent.withValues(alpha: 0.12),
                borderRadius: BorderRadius.circular(8),
                border: Border.all(color: _expertAccent.withValues(alpha: 0.4)),
              ),
              child: Center(
                child: Text(
                  _momentIconText(moment.icon),
                  style: const TextStyle(fontSize: 17),
                ),
              ),
            ),
            const SizedBox(width: 10),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    moment.name,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 14,
                      fontWeight: FontWeight.w800,
                    ),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    _momentSubtitle(moment),
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(
                      color: CelestialColors.textSecondary,
                      fontSize: 11,
                      height: 1.25,
                    ),
                  ),
                ],
              ),
            ),
            const SizedBox(width: 8),
            if (busy)
              const SizedBox(
                width: 20,
                height: 20,
                child: CircularProgressIndicator(strokeWidth: 2),
              )
            else ...[
              IconButton(
                onPressed: onRun,
                tooltip: 'Apply moment',
                icon: const Icon(Icons.play_arrow_rounded),
                color: _expertAccent,
                visualDensity: VisualDensity.compact,
              ),
              IconButton(
                onPressed: onDelete,
                tooltip: 'Delete moment',
                icon: const Icon(Icons.close_rounded),
                color: _dangerAccent,
                visualDensity: VisualDensity.compact,
              ),
              const Icon(
                Icons.chevron_right_rounded,
                color: CelestialColors.textSecondary,
                size: 20,
              ),
            ],
          ],
        ),
      ),
    );
  }
}

class _MomentFieldRow extends StatelessWidget {
  final String label;
  final Widget child;

  const _MomentFieldRow({
    required this.label,
    required this.child,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 10),
      child: Row(
        children: [
          Expanded(
            child: Text(
              label,
              style: const TextStyle(
                color: CelestialColors.textSecondary,
                fontSize: 13,
                fontWeight: FontWeight.w700,
              ),
            ),
          ),
          child,
        ],
      ),
    );
  }
}

class _MomentTimerStepper extends StatelessWidget {
  final int minutes;
  final ValueChanged<int> onChanged;

  const _MomentTimerStepper({
    required this.minutes,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    final safeMinutes = minutes.clamp(0, 480).toInt();
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        _SmallSquareButton(
          icon: Icons.remove_rounded,
          onPressed: safeMinutes <= 0 ? null : () => onChanged(safeMinutes - 5),
        ),
        Container(
          width: 54,
          alignment: Alignment.center,
          child: Text(
            safeMinutes == 0 ? 'off' : _formatMinutes(safeMinutes),
            style: const TextStyle(
              color: CelestialColors.textPrimary,
              fontSize: 12,
              fontWeight: FontWeight.w800,
              fontFeatures: [FontFeature.tabularFigures()],
            ),
          ),
        ),
        _SmallSquareButton(
          icon: Icons.add_rounded,
          onPressed:
              safeMinutes >= 480 ? null : () => onChanged(safeMinutes + 5),
        ),
      ],
    );
  }
}

class _MomentUsageRow extends StatelessWidget {
  final _ExpertMomentAssignment assignment;
  final String areaLabel;
  final bool busy;
  final VoidCallback? onRemove;

  const _MomentUsageRow({
    required this.assignment,
    required this.areaLabel,
    required this.busy,
    required this.onRemove,
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      margin: const EdgeInsets.only(bottom: 8),
      padding: const EdgeInsets.fromLTRB(10, 9, 6, 9),
      decoration: BoxDecoration(
        color: _panel2Color.withValues(alpha: 0.72),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: CelestialColors.orbitRing),
      ),
      child: Row(
        children: [
          Container(
            width: 30,
            height: 30,
            decoration: BoxDecoration(
              color: _expertAccent.withValues(alpha: 0.12),
              borderRadius: BorderRadius.circular(8),
              border: Border.all(color: _expertAccent.withValues(alpha: 0.36)),
            ),
            child: const Icon(
              Icons.settings_remote_rounded,
              color: _expertAccent,
              size: 16,
            ),
          ),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  assignment.control.name,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 13,
                    fontWeight: FontWeight.w800,
                  ),
                ),
                const SizedBox(height: 2),
                Text(
                  '${_buttonEventLabel(assignment.event)} · $areaLabel',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(
                    color: CelestialColors.textSecondary,
                    fontSize: 11,
                    height: 1.25,
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(width: 8),
          if (busy)
            const SizedBox(
              width: 18,
              height: 18,
              child: CircularProgressIndicator(strokeWidth: 2),
            )
          else
            IconButton(
              onPressed: onRemove,
              tooltip: 'Remove assignment',
              icon: const Icon(Icons.close_rounded),
              color: _dangerAccent,
              visualDensity: VisualDensity.compact,
            ),
        ],
      ),
    );
  }
}

class _MomentAssignmentChoice {
  final _ExpertControl control;
  final String event;

  const _MomentAssignmentChoice({
    required this.control,
    required this.event,
  });
}

class _MomentAssignmentSheet extends StatefulWidget {
  final _ExpertMoment moment;
  final List<_ExpertControl> controls;
  final Map<String, _ExpertSwitchType> switchTypes;
  final List<_ExpertMoment> moments;
  final String Function(String areaId) areaName;
  final String Function(String sectionId) sectionName;

  const _MomentAssignmentSheet({
    required this.moment,
    required this.controls,
    required this.switchTypes,
    required this.moments,
    required this.areaName,
    required this.sectionName,
  });

  @override
  State<_MomentAssignmentSheet> createState() => _MomentAssignmentSheetState();
}

class _MomentAssignmentSheetState extends State<_MomentAssignmentSheet> {
  final TextEditingController _queryController = TextEditingController();
  _ExpertControl? _selectedControl;

  @override
  void dispose() {
    _queryController.dispose();
    super.dispose();
  }

  List<_ExpertControl> get _eligibleControls {
    final query = _queryController.text.trim().toLowerCase();
    final controls = [
      for (final control in widget.controls)
        if (control.isSwitch &&
            !control.stale &&
            _momentButtonEventsFor(
              control,
              widget.switchTypes[control.type],
            ).isNotEmpty &&
            _controlHasReachScope(control) &&
            (query.isEmpty ||
                control.name.toLowerCase().contains(query) ||
                control.typeName.toLowerCase().contains(query) ||
                _controlScopeReachLabel(control).toLowerCase().contains(query)))
          control,
    ];
    controls.sort((a, b) => a.name.toLowerCase().compareTo(
          b.name.toLowerCase(),
        ));
    return controls;
  }

  String _controlScopeReachLabel(_ExpertControl control) {
    final names = <String>[];
    for (final scope in control.scopes) {
      for (final areaId in scope.areaIds) {
        final name = widget.areaName(areaId);
        if (!names.contains(name)) names.add(name);
      }
      for (final sectionId in scope.sectionIds) {
        final name = widget.sectionName(sectionId);
        if (!names.contains(name)) names.add(name);
      }
    }
    if (names.isEmpty && control.areaName != null) names.add(control.areaName!);
    if (names.isEmpty) return 'No reach scope';
    if (names.length <= 3) return names.join(', ');
    return '${names.take(3).join(', ')}...';
  }

  bool _controlHasReachScope(_ExpertControl control) {
    for (final scope in control.scopes) {
      if (scope.areaIds.isNotEmpty || scope.sectionIds.isNotEmpty) return true;
    }
    return false;
  }

  @override
  Widget build(BuildContext context) {
    final selected = _selectedControl;
    return SafeArea(
      child: Padding(
        padding: EdgeInsets.only(
          left: 16,
          right: 16,
          top: 14,
          bottom: 16 + MediaQuery.of(context).viewInsets.bottom,
        ),
        child: SizedBox(
          height: MediaQuery.of(context).size.height * 0.72,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Row(
                children: [
                  if (selected != null)
                    IconButton(
                      onPressed: () => setState(() => _selectedControl = null),
                      tooltip: 'Back to controls',
                      icon: const Icon(Icons.arrow_back_rounded),
                      color: CelestialColors.textSecondary,
                      visualDensity: VisualDensity.compact,
                    ),
                  Expanded(
                    child: Text(
                      selected == null ? 'Assign to control' : selected.name,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(
                        color: CelestialColors.textPrimary,
                        fontSize: 18,
                        fontWeight: FontWeight.w800,
                      ),
                    ),
                  ),
                  IconButton(
                    onPressed: () => Navigator.of(context).pop(),
                    tooltip: 'Close',
                    icon: const Icon(Icons.close_rounded),
                    color: CelestialColors.textSecondary,
                    visualDensity: VisualDensity.compact,
                  ),
                ],
              ),
              const SizedBox(height: 10),
              Expanded(
                child: selected == null
                    ? _buildControlList()
                    : _buildSlotList(selected),
              ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _buildControlList() {
    final controls = _eligibleControls;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        TextField(
          controller: _queryController,
          autofocus: true,
          style: const TextStyle(color: CelestialColors.textPrimary),
          decoration: InputDecoration(
            hintText: 'Search controls',
            hintStyle: const TextStyle(color: CelestialColors.textSecondary),
            prefixIcon: const Icon(
              Icons.search_rounded,
              color: CelestialColors.textSecondary,
            ),
            filled: true,
            fillColor: _panel2Color,
            enabledBorder: OutlineInputBorder(
              borderRadius: BorderRadius.circular(8),
              borderSide: const BorderSide(color: CelestialColors.orbitRing),
            ),
            focusedBorder: OutlineInputBorder(
              borderRadius: BorderRadius.circular(8),
              borderSide: const BorderSide(color: _expertAccent),
            ),
          ),
          onChanged: (_) => setState(() {}),
        ),
        const SizedBox(height: 10),
        Expanded(
          child: controls.isEmpty
              ? const Center(
                  child: Text(
                    'No switch controls with assignable buttons.',
                    style: TextStyle(
                      color: CelestialColors.textSecondary,
                      fontSize: 12,
                    ),
                  ),
                )
              : ListView.builder(
                  itemCount: controls.length,
                  itemBuilder: (context, index) {
                    final control = controls[index];
                    return _AssignmentPickerControlRow(
                      control: control,
                      areaLabel: _controlScopeReachLabel(control),
                      assignedCount: control.momentAssignmentCount,
                      onTap: () => setState(() => _selectedControl = control),
                    );
                  },
                ),
        ),
      ],
    );
  }

  Widget _buildSlotList(_ExpertControl control) {
    final events = _momentButtonEventsFor(
      control,
      widget.switchTypes[control.type],
    );
    final actionRef = 'set_${widget.moment.id}';
    return ListView(
      children: [
        Text(
          _controlScopeReachLabel(control),
          maxLines: 2,
          overflow: TextOverflow.ellipsis,
          style: const TextStyle(
            color: CelestialColors.textSecondary,
            fontSize: 12,
            fontWeight: FontWeight.w600,
          ),
        ),
        const SizedBox(height: 10),
        for (final event in events)
          _AssignmentPickerSlotRow(
            event: event,
            currentAction: control.magicButtons[event],
            defaultAction: _magicActionLabel(
              widget.switchTypes[control.type]?.defaultMapping[event],
              widget.moments,
            ),
            currentMoment: actionRef == control.magicButtons[event],
            moments: widget.moments,
            onTap: () => Navigator.of(context).pop(
              _MomentAssignmentChoice(control: control, event: event),
            ),
          ),
      ],
    );
  }
}

class _AssignmentPickerControlRow extends StatelessWidget {
  final _ExpertControl control;
  final String areaLabel;
  final int assignedCount;
  final VoidCallback onTap;

  const _AssignmentPickerControlRow({
    required this.control,
    required this.areaLabel,
    required this.assignedCount,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: onTap,
      behavior: HitTestBehavior.opaque,
      child: Container(
        margin: const EdgeInsets.only(bottom: 8),
        padding: const EdgeInsets.all(10),
        decoration: BoxDecoration(
          color: _panel2Color,
          borderRadius: BorderRadius.circular(8),
          border: Border.all(color: CelestialColors.orbitRing),
        ),
        child: Row(
          children: [
            const Icon(
              Icons.settings_remote_rounded,
              color: _expertAccent,
              size: 18,
            ),
            const SizedBox(width: 10),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    control.name,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 13,
                      fontWeight: FontWeight.w800,
                    ),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    [
                      areaLabel,
                      if (assignedCount > 0) '$assignedCount assigned',
                    ].join(' · '),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(
                      color: CelestialColors.textSecondary,
                      fontSize: 11,
                    ),
                  ),
                ],
              ),
            ),
            const Icon(
              Icons.chevron_right_rounded,
              color: CelestialColors.textSecondary,
              size: 20,
            ),
          ],
        ),
      ),
    );
  }
}

class _AssignmentPickerSlotRow extends StatelessWidget {
  final String event;
  final String? currentAction;
  final String defaultAction;
  final bool currentMoment;
  final List<_ExpertMoment> moments;
  final VoidCallback onTap;

  const _AssignmentPickerSlotRow({
    required this.event,
    required this.currentAction,
    required this.defaultAction,
    required this.currentMoment,
    required this.moments,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    final occupied = currentAction != null && currentAction!.isNotEmpty;
    final subtitle = occupied
        ? _magicAssignmentLabel(currentAction!, moments)
        : defaultAction;
    final color = currentMoment
        ? _changedAccent
        : occupied
            ? _dangerAccent
            : CelestialColors.textSecondary;
    return GestureDetector(
      onTap: onTap,
      behavior: HitTestBehavior.opaque,
      child: Container(
        margin: const EdgeInsets.only(bottom: 8),
        padding: const EdgeInsets.all(10),
        decoration: BoxDecoration(
          color: _panel2Color,
          borderRadius: BorderRadius.circular(8),
          border: Border.all(
            color: currentMoment ? _changedAccent : CelestialColors.orbitRing,
          ),
        ),
        child: Row(
          children: [
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    _buttonEventLabel(event),
                    style: const TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 13,
                      fontWeight: FontWeight.w800,
                    ),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    currentMoment ? 'Assigned to this moment' : subtitle,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      color: color,
                      fontSize: 11,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ],
              ),
            ),
            Icon(
              currentMoment
                  ? Icons.check_circle_rounded
                  : occupied
                      ? Icons.swap_horiz_rounded
                      : Icons.add_circle_outline_rounded,
              color: currentMoment || !occupied ? _expertAccent : _dangerAccent,
              size: 20,
            ),
          ],
        ),
      ),
    );
  }
}

class _MomentIconChoice extends StatelessWidget {
  final String icon;
  final String label;
  final bool selected;
  final VoidCallback onSelected;

  const _MomentIconChoice({
    required this.icon,
    required this.label,
    required this.selected,
    required this.onSelected,
  });

  @override
  Widget build(BuildContext context) {
    return Tooltip(
      message: label,
      child: ChoiceChip(
        label: Text(_momentIconText(icon)),
        selected: selected,
        onSelected: (_) => onSelected(),
        selectedColor: _expertAccent.withValues(alpha: 0.18),
        backgroundColor: _panel2Color,
        side: BorderSide(
          color: selected ? _expertAccent : CelestialColors.orbitRing,
        ),
        labelStyle: TextStyle(
          color: selected ? _expertAccent : CelestialColors.textPrimary,
          fontSize: 17,
          fontWeight: FontWeight.w700,
        ),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
      ),
    );
  }
}

class _MomentExceptionRow extends StatelessWidget {
  final String areaName;
  final _ExpertMomentException exception;
  final ValueChanged<String> onActionChanged;
  final ValueChanged<int> onTimerChanged;
  final VoidCallback onRemove;

  const _MomentExceptionRow({
    required this.areaName,
    required this.exception,
    required this.onActionChanged,
    required this.onTimerChanged,
    required this.onRemove,
  });

  @override
  Widget build(BuildContext context) {
    final minutes = (exception.timerSeconds / 60).round();
    final actionItems = _optionsWithCurrent(
      exception.action,
      [for (final option in _momentActionOptions) option.$1],
    );
    return Container(
      margin: const EdgeInsets.only(bottom: 8),
      padding: const EdgeInsets.all(10),
      decoration: BoxDecoration(
        color: _panel2Color.withValues(alpha: 0.72),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: CelestialColors.orbitRing),
      ),
      child: Column(
        children: [
          Row(
            children: [
              Expanded(
                child: Text(
                  areaName,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 13,
                    fontWeight: FontWeight.w800,
                  ),
                ),
              ),
              IconButton(
                onPressed: onRemove,
                tooltip: 'Remove exception',
                icon: const Icon(Icons.close_rounded),
                color: _dangerAccent,
                visualDensity: VisualDensity.compact,
              ),
            ],
          ),
          Row(
            children: [
              Expanded(
                child: _CompactDropdown<String>(
                  value: exception.action,
                  items: actionItems,
                  itemLabel: _momentActionLabel,
                  onChanged: onActionChanged,
                ),
              ),
              const SizedBox(width: 8),
              _MomentTimerStepper(
                minutes: minutes,
                onChanged: onTimerChanged,
              ),
            ],
          ),
        ],
      ),
    );
  }
}

class _ControlViewChips extends StatelessWidget {
  final String value;
  final ValueChanged<String> onChanged;

  const _ControlViewChips({
    required this.value,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    const modes = [
      ('all', 'Browse'),
      ('setup', 'Setup'),
      ('batteries', 'Batteries'),
      ('paused', 'Paused'),
    ];
    return Wrap(
      spacing: 8,
      runSpacing: 8,
      children: [
        for (final mode in modes)
          ChoiceChip(
            label: Text(mode.$2),
            selected: value == mode.$1,
            onSelected: (_) => onChanged(mode.$1),
            selectedColor: _expertAccent.withValues(alpha: 0.18),
            backgroundColor: _panel2Color,
            side: BorderSide(
              color:
                  value == mode.$1 ? _expertAccent : CelestialColors.orbitRing,
            ),
            labelStyle: TextStyle(
              color: value == mode.$1
                  ? _expertAccent
                  : CelestialColors.textSecondary,
              fontSize: 12,
              fontWeight: FontWeight.w800,
            ),
          ),
      ],
    );
  }
}

class _ControlSortChips extends StatelessWidget {
  final String value;
  final String viewMode;
  final ValueChanged<String> onChanged;

  const _ControlSortChips({
    required this.value,
    required this.viewMode,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    final modes = [
      if (viewMode != 'batteries') ('recent', 'Recent'),
      ('area', 'Location'),
      ('name', 'Name'),
      ('type', 'Type'),
      if (viewMode == 'batteries') ('battery', 'Battery'),
      if (viewMode == 'paused') ('status', 'State'),
    ];
    return Wrap(
      spacing: 8,
      runSpacing: 8,
      children: [
        for (final mode in modes)
          ChoiceChip(
            label: Text(mode.$2),
            selected: value == mode.$1,
            onSelected: (_) => onChanged(mode.$1),
            selectedColor: _expertAccent.withValues(alpha: 0.18),
            backgroundColor: _panel2Color,
            side: BorderSide(
              color:
                  value == mode.$1 ? _expertAccent : CelestialColors.orbitRing,
            ),
            labelStyle: TextStyle(
              color: value == mode.$1
                  ? _expertAccent
                  : CelestialColors.textSecondary,
              fontSize: 12,
              fontWeight: FontWeight.w800,
            ),
          ),
      ],
    );
  }
}

class _ControlCategoryDropdown extends StatelessWidget {
  final String value;
  final List<String> items;
  final ValueChanged<String> onChanged;

  const _ControlCategoryDropdown({
    required this.value,
    required this.items,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    final menuItems = items.contains(value) ? items : [value, ...items];
    return _CompactDropdown<String>(
      value: value,
      items: menuItems,
      itemLabel: (value) =>
          value == 'all' ? 'Types' : _controlCategoryLabel(value),
      onChanged: onChanged,
    );
  }
}

class _SwitchRow extends StatelessWidget {
  final _ExpertControl control;
  final bool busy;
  final int pulseWindowHours;
  final int recentWindowMinutes;
  final VoidCallback? onOpen;
  final VoidCallback onTogglePause;
  final VoidCallback? onDelete;

  const _SwitchRow({
    required this.control,
    required this.busy,
    required this.pulseWindowHours,
    required this.recentWindowMinutes,
    required this.onOpen,
    required this.onTogglePause,
    required this.onDelete,
  });

  @override
  Widget build(BuildContext context) {
    final pulseState = _controlPulseState(
      control.lastActionTime,
      DateTime.now(),
      pulseWindowHours,
      recentWindowMinutes,
    );
    final accent = switch (pulseState) {
      _ControlPulseState.recent => const Color(0xFF22C55E),
      _ControlPulseState.pulse => _changedAccent,
      _ControlPulseState.none => _changedAccent,
    };
    final iconAlpha = switch (pulseState) {
      _ControlPulseState.recent => 0.22,
      _ControlPulseState.pulse => 0.16,
      _ControlPulseState.none => 0.12,
    };
    final borderAlpha = switch (pulseState) {
      _ControlPulseState.recent => 0.72,
      _ControlPulseState.pulse => 0.52,
      _ControlPulseState.none => 0.4,
    };
    return GestureDetector(
      onTap: onOpen,
      behavior: HitTestBehavior.opaque,
      child: Container(
        margin: const EdgeInsets.only(bottom: 8),
        padding: const EdgeInsets.fromLTRB(12, 10, 8, 10),
        decoration: BoxDecoration(
          color: CelestialColors.backgroundCard,
          borderRadius: BorderRadius.circular(8),
          border: Border.all(color: CelestialColors.orbitRing),
        ),
        child: Row(
          children: [
            Container(
              width: 34,
              height: 34,
              decoration: BoxDecoration(
                color: accent.withValues(alpha: iconAlpha),
                borderRadius: BorderRadius.circular(8),
                border:
                    Border.all(color: accent.withValues(alpha: borderAlpha)),
              ),
              child: Icon(
                _controlCategoryIcon(control.category),
                color: accent,
                size: 18,
              ),
            ),
            const SizedBox(width: 10),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      Expanded(
                        child: Text(
                          control.name,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: const TextStyle(
                            color: CelestialColors.textPrimary,
                            fontSize: 14,
                            fontWeight: FontWeight.w800,
                          ),
                        ),
                      ),
                      if (pulseState != _ControlPulseState.none) ...[
                        const SizedBox(width: 6),
                        _ControlPulseDot(state: pulseState),
                      ],
                      if (_controlNeedsIntegration(control.status))
                        const _TinyStatusPill(
                          label: 'INTEGRATE',
                          color: _expertAccent,
                        )
                      else if (control.inactive)
                        _TinyStatusPill(
                          label: _pausePillLabel(control.inactiveUntil),
                          color: _dangerAccent,
                        )
                      else if (control.stale)
                        const _TinyStatusPill(
                          label: 'STALE',
                          color: _dangerAccent,
                        )
                      else if (control.status == 'not_configured')
                        const _TinyStatusPill(
                          label: 'SETUP',
                          color: _expertAccent,
                        )
                      else if (control.batteryLevel != null &&
                          control.batteryLevel! < 10)
                        const _TinyStatusPill(
                          label: 'LOW',
                          color: _dangerAccent,
                        ),
                    ],
                  ),
                  const SizedBox(height: 2),
                  Text(
                    _controlSubtitle(control),
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(
                      color: CelestialColors.textSecondary,
                      fontSize: 11,
                      height: 1.25,
                    ),
                  ),
                ],
              ),
            ),
            const SizedBox(width: 8),
            if (busy)
              const SizedBox(
                width: 20,
                height: 20,
                child: CircularProgressIndicator(strokeWidth: 2),
              )
            else ...[
              IconButton(
                onPressed: onTogglePause,
                tooltip: control.inactive ? 'Resume control' : 'Pause control',
                icon: Icon(
                  control.inactive
                      ? Icons.play_arrow_rounded
                      : Icons.pause_rounded,
                ),
                color:
                    control.inactive ? const Color(0xFF22C55E) : _expertAccent,
                visualDensity: VisualDensity.compact,
              ),
              if (onDelete != null)
                IconButton(
                  onPressed: onDelete,
                  tooltip: 'Delete switch',
                  icon: const Icon(Icons.close_rounded),
                  color: _dangerAccent,
                  visualDensity: VisualDensity.compact,
                ),
              Icon(
                control.isSwitch
                    ? Icons.chevron_right_rounded
                    : Icons.info_outline_rounded,
                color: CelestialColors.textSecondary,
                size: 20,
              ),
            ],
          ],
        ),
      ),
    );
  }
}

class _AreaControlRow extends StatelessWidget {
  final _ExpertControl control;
  final String areaId;
  final Set<String> sectionIds;
  final int pulseWindowHours;
  final int recentWindowMinutes;
  final VoidCallback onOpen;

  const _AreaControlRow({
    required this.control,
    required this.areaId,
    required this.sectionIds,
    required this.pulseWindowHours,
    required this.recentWindowMinutes,
    required this.onOpen,
  });

  @override
  Widget build(BuildContext context) {
    final pulseState = _controlPulseState(
      control.lastActionTime,
      DateTime.now(),
      pulseWindowHours,
      recentWindowMinutes,
    );
    final accent = switch (pulseState) {
      _ControlPulseState.recent => const Color(0xFF22C55E),
      _ControlPulseState.pulse => _expertAccent,
      _ControlPulseState.none => _expertAccent,
    };
    final iconAlpha = switch (pulseState) {
      _ControlPulseState.recent => 0.22,
      _ControlPulseState.pulse => 0.16,
      _ControlPulseState.none => 0.12,
    };
    final borderAlpha = switch (pulseState) {
      _ControlPulseState.recent => 0.72,
      _ControlPulseState.pulse => 0.52,
      _ControlPulseState.none => 0.38,
    };
    return GestureDetector(
      onTap: onOpen,
      behavior: HitTestBehavior.opaque,
      child: Container(
        margin: const EdgeInsets.only(bottom: 8),
        padding: const EdgeInsets.fromLTRB(10, 10, 8, 10),
        decoration: BoxDecoration(
          color: _panel2Color.withValues(alpha: 0.72),
          borderRadius: BorderRadius.circular(8),
          border: Border.all(color: CelestialColors.orbitRing),
        ),
        child: Row(
          children: [
            Container(
              width: 32,
              height: 32,
              decoration: BoxDecoration(
                color: accent.withValues(alpha: iconAlpha),
                borderRadius: BorderRadius.circular(8),
                border: Border.all(
                  color: accent.withValues(alpha: borderAlpha),
                ),
              ),
              child: Icon(
                _controlCategoryIcon(control.category),
                color: accent,
                size: 17,
              ),
            ),
            const SizedBox(width: 10),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      Expanded(
                        child: Text(
                          control.name,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: const TextStyle(
                            color: CelestialColors.textPrimary,
                            fontSize: 13,
                            fontWeight: FontWeight.w800,
                          ),
                        ),
                      ),
                      if (pulseState != _ControlPulseState.none) ...[
                        const SizedBox(width: 6),
                        _ControlPulseDot(state: pulseState),
                      ],
                      if (_controlNeedsIntegration(control.status))
                        const _TinyStatusPill(
                          label: 'INTEGRATE',
                          color: _expertAccent,
                        )
                      else if (control.inactive)
                        _TinyStatusPill(
                          label: _pausePillLabel(control.inactiveUntil),
                          color: _dangerAccent,
                        )
                      else if (control.stale)
                        const _TinyStatusPill(
                          label: 'STALE',
                          color: _dangerAccent,
                        )
                      else if (control.status == 'not_configured')
                        const _TinyStatusPill(
                          label: 'SETUP',
                          color: _expertAccent,
                        ),
                    ],
                  ),
                  const SizedBox(height: 2),
                  Text(
                    _controlSubtitle(control),
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(
                      color: CelestialColors.textSecondary,
                      fontSize: 11,
                      height: 1.25,
                    ),
                  ),
                  const SizedBox(height: 3),
                  Text(
                    _controlAreaReachLabel(
                      control,
                      areaId,
                      sectionIds: sectionIds,
                    ),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(
                      color: _expertAccent,
                      fontSize: 10,
                      fontWeight: FontWeight.w700,
                    ),
                  ),
                ],
              ),
            ),
            const SizedBox(width: 6),
            const Icon(
              Icons.chevron_right_rounded,
              color: CelestialColors.textSecondary,
              size: 20,
            ),
          ],
        ),
      ),
    );
  }
}

class _TinyStatusPill extends StatelessWidget {
  final String label;
  final Color color;

  const _TinyStatusPill({
    required this.label,
    required this.color,
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      margin: const EdgeInsets.only(left: 6),
      padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
      decoration: BoxDecoration(
        color: color.withValues(alpha: 0.14),
        borderRadius: BorderRadius.circular(999),
        border: Border.all(color: color.withValues(alpha: 0.55)),
      ),
      child: Text(
        label,
        style: TextStyle(
          color: color,
          fontSize: 8,
          fontWeight: FontWeight.w900,
          letterSpacing: 0.5,
        ),
      ),
    );
  }
}

class _ControlPulseDot extends StatelessWidget {
  final _ControlPulseState state;

  const _ControlPulseDot({required this.state});

  @override
  Widget build(BuildContext context) {
    final color = switch (state) {
      _ControlPulseState.recent => const Color(0xFF22C55E),
      _ControlPulseState.pulse => _changedAccent,
      _ControlPulseState.none => CelestialColors.textSecondary,
    };
    return Tooltip(
      message: state == _ControlPulseState.recent
          ? 'Recently active'
          : 'Active in pulse window',
      child: Container(
        width: 8,
        height: 8,
        decoration: BoxDecoration(
          color: color,
          shape: BoxShape.circle,
          boxShadow: state == _ControlPulseState.recent
              ? [
                  BoxShadow(
                    color: color.withValues(alpha: 0.45),
                    blurRadius: 8,
                    spreadRadius: 1,
                  ),
                ]
              : null,
        ),
      ),
    );
  }
}

class _ZhaSettingsPanel extends StatelessWidget {
  final _ExpertZhaSettings? settings;
  final bool loading;
  final String? savingKey;
  final ValueChanged<String> onSensitivityChanged;
  final ValueChanged<int> onTimeoutChanged;

  const _ZhaSettingsPanel({
    required this.settings,
    required this.loading,
    required this.savingKey,
    required this.onSensitivityChanged,
    required this.onTimeoutChanged,
  });

  @override
  Widget build(BuildContext context) {
    final sensitivity = settings?.sensitivity;
    final sensitivityOptions = sensitivity?.options ?? const <String>[];
    final timeout = settings?.timeoutSeconds;
    final timeoutOptions = _zhaTimeoutOptions(timeout);

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            const Expanded(
              child: Text(
                'ZHA motion',
                style: TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 15,
                  fontWeight: FontWeight.w800,
                ),
              ),
            ),
            if (loading)
              const SizedBox(
                width: 16,
                height: 16,
                child: CircularProgressIndicator(strokeWidth: 2),
              ),
          ],
        ),
        if (!loading && sensitivity != null && sensitivityOptions.isNotEmpty)
          _ConfigDropdownRow<String>(
            label: 'Sensitivity',
            value: sensitivity.value ?? sensitivityOptions.first,
            items: sensitivityOptions,
            itemLabel: (value) => value,
            saving: savingKey == 'sensitivity',
            onChanged: onSensitivityChanged,
          ),
        if (!loading && timeout != null)
          _ConfigDropdownRow<int>(
            label: 'Timeout',
            value: timeout,
            items: timeoutOptions,
            itemLabel: (value) => '$value sec',
            saving: savingKey == 'timeout',
            onChanged: onTimeoutChanged,
          ),
      ],
    );
  }
}

class _SwitchMagicRow extends StatelessWidget {
  final String event;
  final bool saving;
  final String value;
  final List<String> items;
  final String Function(String id) itemLabel;
  final String defaultAction;
  final bool enabled;
  final ValueChanged<String> onChanged;

  const _SwitchMagicRow({
    required this.event,
    required this.saving,
    required this.value,
    required this.items,
    required this.itemLabel,
    required this.defaultAction,
    this.enabled = true,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      margin: const EdgeInsets.only(bottom: 8),
      padding: const EdgeInsets.all(10),
      decoration: BoxDecoration(
        color: _panel2Color.withValues(alpha: 0.72),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: CelestialColors.orbitRing),
      ),
      child: Row(
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  _buttonEventLabel(event),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 13,
                    fontWeight: FontWeight.w800,
                  ),
                ),
                const SizedBox(height: 2),
                Text(
                  defaultAction,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(
                    color: CelestialColors.textSecondary,
                    fontSize: 11,
                  ),
                ),
              ],
            ),
          ),
          if (saving) ...[
            const SizedBox(width: 8),
            const SizedBox(
              width: 18,
              height: 18,
              child: CircularProgressIndicator(strokeWidth: 2),
            ),
          ] else
            _CompactDropdown<String>(
              value: value,
              items: items,
              itemLabel: itemLabel,
              enabled: enabled,
              onChanged: onChanged,
            ),
        ],
      ),
    );
  }
}

class _SwitchScopeCard extends StatelessWidget {
  final int index;
  final _ExpertSwitchScope scope;
  final List<_ExpertArea> areas;
  final List<_ExpertReachSection> sections;
  final String Function(String areaId) areaName;
  final String Function(String sectionId) sectionName;
  final bool saving;
  final bool enabled;
  final String? selectedAreaId;
  final String? selectedSectionId;
  final ValueChanged<String> onSelectedAreaChanged;
  final ValueChanged<String> onSelectedSectionChanged;
  final ValueChanged<String> onAddArea;
  final ValueChanged<String> onRemoveArea;
  final ValueChanged<String> onAddSection;
  final ValueChanged<String> onRemoveSection;
  final ValueChanged<String> onSetFeedbackArea;
  final bool canMoveUp;
  final bool canMoveDown;
  final VoidCallback onMoveUp;
  final VoidCallback onMoveDown;
  final bool canRemoveScope;
  final VoidCallback onRemoveScope;

  const _SwitchScopeCard({
    required this.index,
    required this.scope,
    required this.areas,
    required this.sections,
    required this.areaName,
    required this.sectionName,
    required this.saving,
    this.enabled = true,
    required this.selectedAreaId,
    required this.selectedSectionId,
    required this.onSelectedAreaChanged,
    required this.onSelectedSectionChanged,
    required this.onAddArea,
    required this.onRemoveArea,
    required this.onAddSection,
    required this.onRemoveSection,
    required this.onSetFeedbackArea,
    required this.canMoveUp,
    required this.canMoveDown,
    required this.onMoveUp,
    required this.onMoveDown,
    required this.canRemoveScope,
    required this.onRemoveScope,
  });

  @override
  Widget build(BuildContext context) {
    final availableAreas = [
      for (final area in areas)
        if (!scope.areaIds.contains(area.id)) area.id,
    ];
    final selected = availableAreas.contains(selectedAreaId)
        ? selectedAreaId!
        : availableAreas.isEmpty
            ? null
            : availableAreas.first;
    final availableSections = [
      for (final section in sections)
        if (!scope.sectionIds.contains(section.id)) section.id,
    ];
    final selectedSection = availableSections.contains(selectedSectionId)
        ? selectedSectionId!
        : availableSections.isEmpty
            ? null
            : availableSections.first;
    final feedbackArea = _scopeFeedbackArea(scope);
    final showFeedbackTargets = scope.areaIds.length > 1;
    final controlsEnabled = enabled && !saving;
    return Container(
      margin: const EdgeInsets.only(bottom: 10),
      padding: const EdgeInsets.all(10),
      decoration: BoxDecoration(
        color: _panel2Color.withValues(alpha: 0.72),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: CelestialColors.orbitRing),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Text(
                'Scope ${index + 1}',
                style: const TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 13,
                  fontWeight: FontWeight.w800,
                ),
              ),
              const Spacer(),
              if (saving)
                const SizedBox(
                  width: 16,
                  height: 16,
                  child: CircularProgressIndicator(strokeWidth: 2),
                )
              else ...[
                _ScopeMoveButton(
                  icon: Icons.keyboard_arrow_up_rounded,
                  tooltip: 'Move scope up',
                  onPressed: controlsEnabled && canMoveUp ? onMoveUp : null,
                ),
                _ScopeMoveButton(
                  icon: Icons.keyboard_arrow_down_rounded,
                  tooltip: 'Move scope down',
                  onPressed: controlsEnabled && canMoveDown ? onMoveDown : null,
                ),
                if (canRemoveScope)
                  IconButton(
                    onPressed: controlsEnabled ? onRemoveScope : null,
                    tooltip: 'Remove scope',
                    icon: const Icon(Icons.close_rounded),
                    color: _dangerAccent,
                    visualDensity: VisualDensity.compact,
                  ),
              ],
            ],
          ),
          if (scope.areaIds.isEmpty && scope.sectionIds.isEmpty)
            const Padding(
              padding: EdgeInsets.only(bottom: 8),
              child: Text(
                'No targets yet.',
                style: TextStyle(
                  color: CelestialColors.textSecondary,
                  fontSize: 12,
                ),
              ),
            )
          else ...[
            Wrap(
              spacing: 6,
              runSpacing: 6,
              children: [
                for (final areaId in scope.areaIds)
                  _ScopeAreaChip(
                    label: Text(areaName(areaId)),
                    showFeedbackTarget: showFeedbackTargets,
                    feedbackTarget: areaId == feedbackArea,
                    saving: !controlsEnabled,
                    onSetFeedbackTarget: () => onSetFeedbackArea(areaId),
                    onDeleted: () => onRemoveArea(areaId),
                  ),
                for (final sectionId in scope.sectionIds)
                  _ScopeSectionChip(
                    label: Text(sectionName(sectionId)),
                    saving: !controlsEnabled,
                    onDeleted: () => onRemoveSection(sectionId),
                  ),
              ],
            ),
            const SizedBox(height: 10),
          ],
          if (selected != null)
            Wrap(
              spacing: 8,
              runSpacing: 8,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                _CompactDropdown<String>(
                  value: selected,
                  items: availableAreas,
                  itemLabel: areaName,
                  enabled: controlsEnabled,
                  onChanged: onSelectedAreaChanged,
                ),
                OutlinedButton.icon(
                  onPressed: controlsEnabled ? () => onAddArea(selected) : null,
                  icon: const Icon(Icons.add_rounded, size: 17),
                  label: const Text('Add area'),
                  style: OutlinedButton.styleFrom(
                    foregroundColor: _expertAccent,
                    side: const BorderSide(color: CelestialColors.orbitRing),
                    shape: RoundedRectangleBorder(
                      borderRadius: BorderRadius.circular(8),
                    ),
                  ),
                ),
              ],
            ),
          if (selectedSection != null) ...[
            if (selected != null) const SizedBox(height: 8),
            Wrap(
              spacing: 8,
              runSpacing: 8,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                _CompactDropdown<String>(
                  value: selectedSection,
                  items: availableSections,
                  itemLabel: sectionName,
                  enabled: controlsEnabled,
                  onChanged: onSelectedSectionChanged,
                ),
                OutlinedButton.icon(
                  onPressed: controlsEnabled
                      ? () => onAddSection(selectedSection)
                      : null,
                  icon: const Icon(Icons.add_rounded, size: 17),
                  label: const Text('Add section'),
                  style: OutlinedButton.styleFrom(
                    foregroundColor: _changedAccent,
                    side: const BorderSide(color: CelestialColors.orbitRing),
                    shape: RoundedRectangleBorder(
                      borderRadius: BorderRadius.circular(8),
                    ),
                  ),
                ),
              ],
            ),
          ],
        ],
      ),
    );
  }
}

class _ControlScopeCard extends StatelessWidget {
  final int index;
  final _ExpertSwitchScope scope;
  final List<_ExpertArea> areas;
  final List<_ExpertReachSection> sections;
  final List<_ExpertTriggerEntity> triggerEntities;
  final String Function(String areaId) areaName;
  final String Function(String sectionId) sectionName;
  final bool saving;
  final bool enabled;
  final String? selectedAreaId;
  final String? selectedSectionId;
  final ValueChanged<String> onSelectedAreaChanged;
  final ValueChanged<String> onSelectedSectionChanged;
  final ValueChanged<String> onAddArea;
  final ValueChanged<String> onRemoveArea;
  final ValueChanged<String> onAddSection;
  final ValueChanged<String> onRemoveSection;
  final ValueChanged<_ExpertSwitchScope> onUpdate;
  final bool canMoveUp;
  final bool canMoveDown;
  final VoidCallback onMoveUp;
  final VoidCallback onMoveDown;
  final bool canRemoveScope;
  final VoidCallback onRemoveScope;

  const _ControlScopeCard({
    required this.index,
    required this.scope,
    required this.areas,
    required this.sections,
    required this.triggerEntities,
    required this.areaName,
    required this.sectionName,
    required this.saving,
    this.enabled = true,
    required this.selectedAreaId,
    required this.selectedSectionId,
    required this.onSelectedAreaChanged,
    required this.onSelectedSectionChanged,
    required this.onAddArea,
    required this.onRemoveArea,
    required this.onAddSection,
    required this.onRemoveSection,
    required this.onUpdate,
    required this.canMoveUp,
    required this.canMoveDown,
    required this.onMoveUp,
    required this.onMoveDown,
    required this.canRemoveScope,
    required this.onRemoveScope,
  });

  @override
  Widget build(BuildContext context) {
    final availableAreas = [
      for (final area in areas)
        if (!scope.areaIds.contains(area.id)) area.id,
    ];
    final selected = availableAreas.contains(selectedAreaId)
        ? selectedAreaId!
        : availableAreas.isEmpty
            ? null
            : availableAreas.first;
    final availableSections = [
      for (final section in sections)
        if (!scope.sectionIds.contains(section.id)) section.id,
    ];
    final selectedSection = availableSections.contains(selectedSectionId)
        ? selectedSectionId!
        : availableSections.isEmpty
            ? null
            : availableSections.first;
    final showDuration = scope.mode == 'on_off';
    final showAlert = scope.mode == 'alert';
    final showBoost = scope.mode != 'alert' && scope.mode != 'disabled';
    final showOffset = scope.activeWhen != 'always';
    final controlsEnabled = enabled && !saving;

    return Container(
      margin: const EdgeInsets.only(bottom: 10),
      padding: const EdgeInsets.all(10),
      decoration: BoxDecoration(
        color: _panel2Color.withValues(alpha: 0.72),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: CelestialColors.orbitRing),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(
                  _controlScopeTitle(index, scope, areaName, sectionName),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 13,
                    fontWeight: FontWeight.w800,
                  ),
                ),
              ),
              if (saving)
                const SizedBox(
                  width: 16,
                  height: 16,
                  child: CircularProgressIndicator(strokeWidth: 2),
                )
              else ...[
                _ScopeMoveButton(
                  icon: Icons.keyboard_arrow_up_rounded,
                  tooltip: 'Move reach up',
                  onPressed: controlsEnabled && canMoveUp ? onMoveUp : null,
                ),
                _ScopeMoveButton(
                  icon: Icons.keyboard_arrow_down_rounded,
                  tooltip: 'Move reach down',
                  onPressed: controlsEnabled && canMoveDown ? onMoveDown : null,
                ),
                if (canRemoveScope)
                  IconButton(
                    onPressed: controlsEnabled ? onRemoveScope : null,
                    tooltip: 'Remove reach',
                    icon: const Icon(Icons.close_rounded),
                    color: _dangerAccent,
                    visualDensity: VisualDensity.compact,
                  ),
              ],
            ],
          ),
          const SizedBox(height: 6),
          if (scope.areaIds.isEmpty && scope.sectionIds.isEmpty)
            const Padding(
              padding: EdgeInsets.only(bottom: 8),
              child: Text(
                'No targets yet.',
                style: TextStyle(
                  color: CelestialColors.textSecondary,
                  fontSize: 12,
                ),
              ),
            )
          else ...[
            Wrap(
              spacing: 6,
              runSpacing: 6,
              children: [
                for (final areaId in scope.areaIds)
                  _ScopeAreaChip(
                    label: Text(areaName(areaId)),
                    showFeedbackTarget: false,
                    feedbackTarget: false,
                    saving: !controlsEnabled,
                    onSetFeedbackTarget: () {},
                    onDeleted: () => onRemoveArea(areaId),
                  ),
                for (final sectionId in scope.sectionIds)
                  _ScopeSectionChip(
                    label: Text(sectionName(sectionId)),
                    saving: !controlsEnabled,
                    onDeleted: () => onRemoveSection(sectionId),
                  ),
              ],
            ),
            const SizedBox(height: 10),
          ],
          if (selected != null)
            Wrap(
              spacing: 8,
              runSpacing: 8,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                _CompactDropdown<String>(
                  value: selected,
                  items: availableAreas,
                  itemLabel: areaName,
                  enabled: controlsEnabled,
                  onChanged: onSelectedAreaChanged,
                ),
                OutlinedButton.icon(
                  onPressed: controlsEnabled ? () => onAddArea(selected) : null,
                  icon: const Icon(Icons.add_rounded, size: 17),
                  label: const Text('Add area'),
                  style: OutlinedButton.styleFrom(
                    foregroundColor: _expertAccent,
                    side: const BorderSide(color: CelestialColors.orbitRing),
                    shape: RoundedRectangleBorder(
                      borderRadius: BorderRadius.circular(8),
                    ),
                  ),
                ),
              ],
            ),
          if (selectedSection != null) ...[
            if (selected != null) const SizedBox(height: 8),
            Wrap(
              spacing: 8,
              runSpacing: 8,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                _CompactDropdown<String>(
                  value: selectedSection,
                  items: availableSections,
                  itemLabel: sectionName,
                  enabled: controlsEnabled,
                  onChanged: onSelectedSectionChanged,
                ),
                OutlinedButton.icon(
                  onPressed: controlsEnabled
                      ? () => onAddSection(selectedSection)
                      : null,
                  icon: const Icon(Icons.add_rounded, size: 17),
                  label: const Text('Add section'),
                  style: OutlinedButton.styleFrom(
                    foregroundColor: _changedAccent,
                    side: const BorderSide(color: CelestialColors.orbitRing),
                    shape: RoundedRectangleBorder(
                      borderRadius: BorderRadius.circular(8),
                    ),
                  ),
                ),
              ],
            ),
          ],
          const SizedBox(height: 10),
          _MomentFieldRow(
            label: 'Action',
            child: _CompactDropdown<String>(
              value: scope.mode,
              items: _optionsWithCurrent(
                scope.mode,
                const ['on_only', 'on_off', 'alert', 'disabled'],
              ),
              itemLabel: _sensorModeLabel,
              enabled: controlsEnabled,
              onChanged: (value) => onUpdate(scope.copyWith(mode: value)),
            ),
          ),
          if (triggerEntities.isNotEmpty) ...[
            const SizedBox(height: 8),
            _ControlTriggerSection(
              sensors: triggerEntities,
              selected: scope.triggerEntities,
              saving: saving,
              enabled: controlsEnabled,
              onChanged: (entities) => onUpdate(
                scope.copyWith(triggerEntities: entities),
              ),
            ),
          ],
          if (showDuration)
            _ConfigNumberRow(
              label: 'Timer',
              unit: 'min',
              value: scope.durationSeconds / 60,
              min: 0.5,
              max: 60,
              step: 0.5,
              saving: saving,
              enabled: controlsEnabled,
              onChanged: (value) => onUpdate(
                scope.copyWith(durationSeconds: (value * 60).round()),
              ),
            ),
          if (showBoost) ...[
            _ConfigToggleRow(
              label: 'Boost',
              value: scope.boostEnabled,
              saving: saving,
              enabled: controlsEnabled,
              onChanged: (value) => onUpdate(
                scope.copyWith(boostEnabled: value),
              ),
            ),
            if (scope.boostEnabled)
              _ConfigNumberRow(
                label: 'Boost brightness',
                unit: '%',
                value: scope.boostBrightness,
                min: 5,
                max: 100,
                step: 5,
                saving: saving,
                enabled: controlsEnabled,
                onChanged: (value) => onUpdate(
                  scope.copyWith(boostBrightness: value.round()),
                ),
              ),
          ],
          if (showAlert) ...[
            _MomentFieldRow(
              label: 'Alert strength',
              child: _CompactDropdown<String>(
                value: scope.alertIntensity,
                items: const ['low', 'med', 'high'],
                itemLabel: _alertIntensityLabel,
                enabled: controlsEnabled,
                onChanged: (value) =>
                    onUpdate(scope.copyWith(alertIntensity: value)),
              ),
            ),
            _ConfigNumberRow(
              label: 'Bounces',
              unit: 'x',
              value: scope.alertCount,
              min: 1,
              max: 10,
              step: 1,
              saving: saving,
              enabled: controlsEnabled,
              onChanged: (value) => onUpdate(
                scope.copyWith(alertCount: value.round()),
              ),
            ),
          ],
          _MomentFieldRow(
            label: 'Schedule',
            child: _CompactDropdown<String>(
              value: scope.activeWhen,
              items: const ['always', 'sunset_to_sunrise', 'wake_to_bed'],
              itemLabel: _sensorScheduleLabel,
              enabled: controlsEnabled,
              onChanged: (value) => onUpdate(
                scope.copyWith(activeWhen: value),
              ),
            ),
          ),
          if (showOffset)
            _ConfigNumberRow(
              label: 'Offset',
              unit: 'min',
              value: scope.activeOffset,
              min: -120,
              max: 120,
              step: 5,
              saving: saving,
              enabled: controlsEnabled,
              onChanged: (value) => onUpdate(
                scope.copyWith(activeOffset: value.round()),
              ),
            ),
          _ConfigNumberRow(
            label: 'Cooloff',
            unit: 's',
            value: scope.cooldownSeconds,
            min: 0,
            max: 300,
            step: 5,
            saving: saving,
            enabled: controlsEnabled,
            onChanged: (value) => onUpdate(
              scope.copyWith(cooldownSeconds: value.round()),
            ),
          ),
        ],
      ),
    );
  }
}

class _ScopeAreaChip extends StatelessWidget {
  final Widget label;
  final bool showFeedbackTarget;
  final bool feedbackTarget;
  final bool saving;
  final VoidCallback onSetFeedbackTarget;
  final VoidCallback onDeleted;

  const _ScopeAreaChip({
    required this.label,
    required this.showFeedbackTarget,
    required this.feedbackTarget,
    required this.saving,
    required this.onSetFeedbackTarget,
    required this.onDeleted,
  });

  @override
  Widget build(BuildContext context) {
    final active = showFeedbackTarget && feedbackTarget;
    final chip = InputChip(
      avatar: showFeedbackTarget
          ? Icon(
              active ? Icons.star_rounded : Icons.star_border_rounded,
              color: active ? _expertAccent : CelestialColors.textSecondary,
              size: 16,
            )
          : null,
      label: label,
      selected: active,
      showCheckmark: false,
      onSelected:
          showFeedbackTarget && !saving ? (_) => onSetFeedbackTarget() : null,
      onDeleted: saving ? null : onDeleted,
      backgroundColor: _panelColor,
      selectedColor: _expertAccent.withValues(alpha: 0.16),
      deleteIconColor: _dangerAccent,
      side: BorderSide(
        color: active ? _expertAccent : CelestialColors.orbitRing,
      ),
      labelStyle: TextStyle(
        color: active ? _expertAccent : CelestialColors.textPrimary,
        fontSize: 12,
        fontWeight: FontWeight.w700,
      ),
    );
    if (!showFeedbackTarget) return chip;
    return Tooltip(
      message: active ? 'Feedback target' : 'Use for feedback',
      child: chip,
    );
  }
}

class _ScopeMoveButton extends StatelessWidget {
  final IconData icon;
  final String tooltip;
  final VoidCallback? onPressed;

  const _ScopeMoveButton({
    required this.icon,
    required this.tooltip,
    required this.onPressed,
  });

  @override
  Widget build(BuildContext context) {
    return IconButton(
      onPressed: onPressed,
      tooltip: tooltip,
      icon: Icon(icon),
      color: CelestialColors.textSecondary,
      disabledColor: CelestialColors.textSecondary.withValues(alpha: 0.24),
      visualDensity: VisualDensity.compact,
      iconSize: 19,
      padding: EdgeInsets.zero,
      constraints: const BoxConstraints.tightFor(width: 28, height: 28),
    );
  }
}

class _ScopeSectionChip extends StatelessWidget {
  final Widget label;
  final bool saving;
  final VoidCallback onDeleted;

  const _ScopeSectionChip({
    required this.label,
    required this.saving,
    required this.onDeleted,
  });

  @override
  Widget build(BuildContext context) {
    return InputChip(
      avatar: const Icon(
        Icons.splitscreen_rounded,
        color: _changedAccent,
        size: 15,
      ),
      label: label,
      onDeleted: saving ? null : onDeleted,
      backgroundColor: _panelColor,
      deleteIconColor: _dangerAccent,
      side: const BorderSide(color: CelestialColors.orbitRing),
      labelStyle: const TextStyle(
        color: CelestialColors.textPrimary,
        fontSize: 12,
        fontWeight: FontWeight.w700,
      ),
    );
  }
}

class _ControlTriggerSection extends StatelessWidget {
  final List<_ExpertTriggerEntity> sensors;
  final List<String> selected;
  final bool saving;
  final bool enabled;
  final ValueChanged<List<String>> onChanged;

  const _ControlTriggerSection({
    required this.sensors,
    required this.selected,
    required this.saving,
    this.enabled = true,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    final groups = _triggerGroupsForSensors(sensors);
    final categorized = {
      for (final group in groups) ...group.allEntityIds,
    };
    final otherSensors = [
      for (final sensor in sensors)
        if (!categorized.contains(sensor.entityId)) sensor,
    ];
    if (groups.isEmpty && otherSensors.isEmpty) return const SizedBox.shrink();

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const Text(
          'Presence',
          style: TextStyle(
            color: CelestialColors.textSecondary,
            fontSize: 11,
            fontWeight: FontWeight.w800,
          ),
        ),
        const SizedBox(height: 6),
        Wrap(
          spacing: 6,
          runSpacing: 6,
          children: [
            for (final group in groups)
              FilterChip(
                label: Text(group.label),
                selected: _triggerGroupSelected(selected, group),
                onSelected: saving || !enabled
                    ? null
                    : (_) => onChanged(_toggleTriggerGroup(selected, group)),
                selectedColor: _expertAccent.withValues(alpha: 0.2),
                backgroundColor: _panelColor,
                side: BorderSide(
                  color: _triggerGroupSelected(selected, group)
                      ? _expertAccent
                      : CelestialColors.orbitRing,
                ),
                labelStyle: TextStyle(
                  color: _triggerGroupSelected(selected, group)
                      ? _expertAccent
                      : CelestialColors.textSecondary,
                  fontSize: 11,
                  fontWeight: FontWeight.w800,
                ),
              ),
            for (final sensor in otherSensors)
              FilterChip(
                label: Text(sensor.name),
                selected: selected.contains(sensor.entityId),
                onSelected: saving || !enabled
                    ? null
                    : (_) => onChanged(
                          _toggleTriggerEntity(selected, sensor.entityId),
                        ),
                selectedColor: _changedAccent.withValues(alpha: 0.18),
                backgroundColor: _panelColor,
                side: BorderSide(
                  color: selected.contains(sensor.entityId)
                      ? _changedAccent
                      : CelestialColors.orbitRing,
                ),
                labelStyle: TextStyle(
                  color: selected.contains(sensor.entityId)
                      ? _changedAccent
                      : CelestialColors.textSecondary,
                  fontSize: 11,
                  fontWeight: FontWeight.w800,
                ),
              ),
          ],
        ),
      ],
    );
  }
}

class _SwitchmapButtonCard extends StatelessWidget {
  final String button;
  final List<String> actionTypes;
  final Map<String, Object?> draftMappings;
  final List<String> Function(String actionType) actionItems;
  final String Function(String id) actionLabel;
  final List<String> whenOffItems;
  final String Function(String id) whenOffLabel;
  final bool enabled;
  final void Function(String eventKey, String value) onActionChanged;
  final void Function(String eventKey, String value) onWhenOffChanged;

  const _SwitchmapButtonCard({
    required this.button,
    required this.actionTypes,
    required this.draftMappings,
    required this.actionItems,
    required this.actionLabel,
    required this.whenOffItems,
    required this.whenOffLabel,
    this.enabled = true,
    required this.onActionChanged,
    required this.onWhenOffChanged,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(bottom: 10),
      child: _ExpertPanel(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(
              children: [
                Container(
                  width: 28,
                  height: 28,
                  alignment: Alignment.center,
                  decoration: BoxDecoration(
                    color: _expertAccent.withValues(alpha: 0.14),
                    borderRadius: BorderRadius.circular(7),
                    border: Border.all(
                      color: _expertAccent.withValues(alpha: 0.55),
                    ),
                  ),
                  child: Text(
                    _buttonBadge(button),
                    style: const TextStyle(
                      color: _expertAccent,
                      fontSize: 11,
                      fontWeight: FontWeight.w900,
                    ),
                  ),
                ),
                const SizedBox(width: 10),
                Text(
                  _buttonName(button),
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 15,
                    fontWeight: FontWeight.w800,
                  ),
                ),
              ],
            ),
            const SizedBox(height: 10),
            if (actionTypes.isEmpty)
              const Text(
                'No displayable actions for this button.',
                style: TextStyle(
                  color: CelestialColors.textSecondary,
                  fontSize: 12,
                ),
              )
            else
              for (final actionType in actionTypes)
                _SwitchmapActionRow(
                  eventKey: '${button}_$actionType',
                  actionType: actionType,
                  mapping: draftMappings['${button}_$actionType'],
                  actionItems: actionItems(actionType),
                  actionLabel: actionLabel,
                  whenOffItems: whenOffItems,
                  whenOffLabel: whenOffLabel,
                  enabled: enabled,
                  onActionChanged: onActionChanged,
                  onWhenOffChanged: onWhenOffChanged,
                ),
          ],
        ),
      ),
    );
  }
}

class _SwitchmapActionRow extends StatelessWidget {
  final String eventKey;
  final String actionType;
  final Object? mapping;
  final List<String> actionItems;
  final String Function(String id) actionLabel;
  final List<String> whenOffItems;
  final String Function(String id) whenOffLabel;
  final bool enabled;
  final void Function(String eventKey, String value) onActionChanged;
  final void Function(String eventKey, String value) onWhenOffChanged;

  const _SwitchmapActionRow({
    required this.eventKey,
    required this.actionType,
    required this.mapping,
    required this.actionItems,
    required this.actionLabel,
    required this.whenOffItems,
    required this.whenOffLabel,
    this.enabled = true,
    required this.onActionChanged,
    required this.onWhenOffChanged,
  });

  @override
  Widget build(BuildContext context) {
    final action = _mappingMainAction(mapping);
    final actionValue = action ?? '__none__';
    final supportsWhenOff = _isAdjustmentAction(action);
    final whenOff = _mappingWhenOff(mapping) ?? '__none__';
    final safeActionItems = actionItems.contains(actionValue)
        ? actionItems
        : [actionValue, ...actionItems];
    final safeWhenOffItems = whenOffItems.contains(whenOff)
        ? whenOffItems
        : [whenOff, ...whenOffItems];

    return Container(
      margin: const EdgeInsets.only(bottom: 8),
      padding: const EdgeInsets.all(10),
      decoration: BoxDecoration(
        color: _panel2Color.withValues(alpha: 0.72),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: CelestialColors.orbitRing),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(
                  _actionTypeLabel(actionType),
                  style: const TextStyle(
                    color: CelestialColors.textSecondary,
                    fontSize: 12,
                    fontWeight: FontWeight.w800,
                  ),
                ),
              ),
              _CompactDropdown<String>(
                value: actionValue,
                items: safeActionItems,
                itemLabel: actionLabel,
                enabled: enabled,
                onChanged: (value) => onActionChanged(eventKey, value),
              ),
            ],
          ),
          if (supportsWhenOff) ...[
            const SizedBox(height: 8),
            Row(
              children: [
                const Expanded(
                  child: Text(
                    'If off',
                    style: TextStyle(
                      color: CelestialColors.textSecondary,
                      fontSize: 12,
                      fontWeight: FontWeight.w700,
                    ),
                  ),
                ),
                _CompactDropdown<String>(
                  value: whenOff,
                  items: safeWhenOffItems,
                  itemLabel: whenOffLabel,
                  enabled: enabled,
                  onChanged: (value) => onWhenOffChanged(eventKey, value),
                ),
              ],
            ),
          ],
        ],
      ),
    );
  }
}

class _HistoryPreview extends StatelessWidget {
  final List<_ExpertHistoryEntry> entries;
  final String? hint;
  final VoidCallback onRefresh;

  const _HistoryPreview({
    required this.entries,
    required this.hint,
    required this.onRefresh,
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      decoration: BoxDecoration(
        color: _panel2Color.withValues(alpha: 0.45),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: CelestialColors.orbitRing),
      ),
      child: Column(
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(12, 10, 6, 8),
            child: Row(
              children: [
                const Expanded(
                  child: Text(
                    'History',
                    style: TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 14,
                      fontWeight: FontWeight.w700,
                    ),
                  ),
                ),
                IconButton(
                  onPressed: onRefresh,
                  tooltip: 'Refresh history',
                  icon: const Icon(Icons.refresh_rounded),
                  iconSize: 18,
                  visualDensity: VisualDensity.compact,
                  color: CelestialColors.textSecondary,
                ),
              ],
            ),
          ),
          if (entries.isEmpty)
            Padding(
              padding: const EdgeInsets.fromLTRB(12, 0, 12, 12),
              child: Text(
                'No recent area events.',
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.85),
                  fontSize: 12,
                ),
              ),
            )
          else
            for (final entry in entries.take(5)) _HistoryRow(entry: entry),
          if (hint != null)
            Padding(
              padding: const EdgeInsets.fromLTRB(12, 8, 12, 12),
              child: Text(
                hint!,
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.75),
                  fontSize: 11,
                ),
              ),
            ),
        ],
      ),
    );
  }
}

class _HistoryRow extends StatelessWidget {
  final _ExpertHistoryEntry entry;

  const _HistoryRow({required this.entry});

  @override
  Widget build(BuildContext context) {
    final dotColor = entry.isNow ? _changedAccent : _expertAccent;
    return Padding(
      padding: const EdgeInsets.fromLTRB(12, 0, 12, 8),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Container(
            width: 7,
            height: 7,
            margin: const EdgeInsets.only(top: 5),
            decoration: BoxDecoration(
              color: dotColor,
              shape: BoxShape.circle,
            ),
          ),
          const SizedBox(width: 8),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  entry.title,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 12,
                    fontWeight: FontWeight.w700,
                  ),
                ),
                if (entry.subtitle != null)
                  Text(
                    entry.subtitle!,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(
                      color: CelestialColors.textSecondary,
                      fontSize: 11,
                    ),
                  ),
              ],
            ),
          ),
          const SizedBox(width: 8),
          Text(
            entry.timeLabel,
            style: const TextStyle(
              color: CelestialColors.textSecondary,
              fontSize: 11,
              fontFeatures: [FontFeature.tabularFigures()],
            ),
          ),
        ],
      ),
    );
  }
}

class _LightGroupHeader extends StatelessWidget {
  final String name;
  final int count;

  const _LightGroupHeader({
    required this.name,
    required this.count,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(top: 8, bottom: 4),
      child: Row(
        children: [
          Expanded(
            child: Text(
              name,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: const TextStyle(
                color: CelestialColors.textSecondary,
                fontSize: 12,
                fontWeight: FontWeight.w800,
              ),
            ),
          ),
          Text(
            '$count',
            style: const TextStyle(
              color: CelestialColors.textSecondary,
              fontSize: 11,
              fontWeight: FontWeight.w700,
              fontFeatures: [FontFeature.tabularFigures()],
            ),
          ),
        ],
      ),
    );
  }
}

class _LightPurposeRow extends StatelessWidget {
  final _ExpertLightRow light;
  final List<_ExpertSectionOption> sections;
  final List<String> options;
  final bool dirty;
  final bool saving;
  final bool identifying;
  final bool sectionEnabled;
  final bool purposeEnabled;
  final ValueChanged<String?> onSectionChanged;
  final ValueChanged<String> onChanged;
  final VoidCallback onIdentify;

  const _LightPurposeRow({
    required this.light,
    required this.sections,
    required this.options,
    required this.dirty,
    required this.saving,
    required this.identifying,
    this.sectionEnabled = true,
    this.purposeEnabled = true,
    required this.onSectionChanged,
    required this.onChanged,
    required this.onIdentify,
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      margin: const EdgeInsets.only(top: 6),
      padding: const EdgeInsets.fromLTRB(10, 8, 8, 8),
      decoration: BoxDecoration(
        color: _panel2Color.withValues(alpha: 0.56),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(
          color: dirty
              ? _expertAccent.withValues(alpha: 0.62)
              : CelestialColors.orbitRing,
        ),
      ),
      child: Row(
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  light.name,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 13,
                    fontWeight: FontWeight.w700,
                  ),
                ),
                const SizedBox(height: 2),
                Text(
                  light.entityId,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(
                    color: CelestialColors.textSecondary,
                    fontSize: 10,
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(width: 10),
          if (saving)
            const SizedBox(
              width: 18,
              height: 18,
              child: CircularProgressIndicator(strokeWidth: 2),
            )
          else ...[
            IconButton(
              onPressed: identifying ? null : onIdentify,
              tooltip: 'Identify light',
              icon: identifying
                  ? const SizedBox(
                      width: 16,
                      height: 16,
                      child: CircularProgressIndicator(strokeWidth: 2),
                    )
                  : const Icon(Icons.flashlight_on_rounded),
              iconSize: 18,
              style: IconButton.styleFrom(
                foregroundColor: _expertAccent,
                disabledForegroundColor:
                    CelestialColors.textSecondary.withValues(alpha: 0.45),
                backgroundColor: _panelColor,
                fixedSize: const Size(34, 34),
                minimumSize: const Size(34, 34),
                padding: EdgeInsets.zero,
                shape: RoundedRectangleBorder(
                  borderRadius: BorderRadius.circular(7),
                ),
                side: const BorderSide(color: CelestialColors.orbitRing),
              ),
            ),
            const SizedBox(width: 8),
            _LightSectionDropdown(
              value: light.sectionId,
              sections: sections,
              enabled: sectionEnabled,
              onChanged: (sectionId) {
                if (sectionId != light.sectionId) {
                  onSectionChanged(sectionId);
                }
              },
            ),
            const SizedBox(width: 8),
            _CompactDropdown<String>(
              value: light.purpose,
              items: options,
              itemLabel: (option) => option,
              enabled: purposeEnabled,
              onChanged: (value) {
                if (value != light.purpose) onChanged(value);
              },
            ),
          ],
        ],
      ),
    );
  }
}

class _SectionManagementRows extends StatelessWidget {
  final List<_ExpertSectionOption> sections;
  final String? savingKey;
  final bool enabled;
  final VoidCallback onCreate;
  final ValueChanged<_ExpertSectionOption> onRename;
  final ValueChanged<_ExpertSectionOption> onDelete;

  const _SectionManagementRows({
    required this.sections,
    required this.savingKey,
    this.enabled = true,
    required this.onCreate,
    required this.onRename,
    required this.onDelete,
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.fromLTRB(10, 8, 8, 8),
      decoration: BoxDecoration(
        color: _panelColor.withValues(alpha: 0.62),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: CelestialColors.orbitRing),
      ),
      child: Column(
        children: [
          Row(
            children: [
              const Expanded(
                child: Text(
                  'Sections',
                  style: TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 13,
                    fontWeight: FontWeight.w800,
                  ),
                ),
              ),
              if (savingKey == '__create__')
                const SizedBox(
                  width: 18,
                  height: 18,
                  child: CircularProgressIndicator(strokeWidth: 2),
                )
              else
                IconButton(
                  onPressed: enabled ? onCreate : null,
                  tooltip: 'New section',
                  icon: const Icon(Icons.add_rounded),
                  color: _expertAccent,
                  visualDensity: VisualDensity.compact,
                ),
            ],
          ),
          if (sections.isEmpty)
            const Padding(
              padding: EdgeInsets.only(top: 2, bottom: 4),
              child: Align(
                alignment: Alignment.centerLeft,
                child: Text(
                  'Main section only',
                  style: TextStyle(
                    color: CelestialColors.textSecondary,
                    fontSize: 11,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
            )
          else
            for (final section in sections)
              _SectionManagementRow(
                section: section,
                saving: savingKey == section.id,
                enabled: enabled,
                onRename: () => onRename(section),
                onDelete: () => onDelete(section),
              ),
        ],
      ),
    );
  }
}

class _SectionManagementRow extends StatelessWidget {
  final _ExpertSectionOption section;
  final bool saving;
  final bool enabled;
  final VoidCallback onRename;
  final VoidCallback onDelete;

  const _SectionManagementRow({
    required this.section,
    required this.saving,
    this.enabled = true,
    required this.onRename,
    required this.onDelete,
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      margin: const EdgeInsets.only(top: 6),
      padding: const EdgeInsets.fromLTRB(8, 5, 4, 5),
      decoration: BoxDecoration(
        color: _panel2Color.withValues(alpha: 0.46),
        borderRadius: BorderRadius.circular(7),
        border: Border.all(
          color: CelestialColors.orbitRing.withValues(alpha: 0.75),
        ),
      ),
      child: Row(
        children: [
          Expanded(
            child: Text(
              section.name,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: const TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 12,
                fontWeight: FontWeight.w700,
              ),
            ),
          ),
          if (saving)
            const SizedBox(
              width: 18,
              height: 18,
              child: CircularProgressIndicator(strokeWidth: 2),
            )
          else ...[
            IconButton(
              onPressed: enabled ? onRename : null,
              tooltip: 'Rename section',
              icon: const Icon(Icons.edit_rounded),
              iconSize: 16,
              color: CelestialColors.textSecondary,
              visualDensity: VisualDensity.compact,
            ),
            IconButton(
              onPressed: enabled ? onDelete : null,
              tooltip: 'Delete section',
              icon: const Icon(Icons.delete_outline_rounded),
              iconSize: 16,
              color: _dangerAccent,
              visualDensity: VisualDensity.compact,
            ),
          ],
        ],
      ),
    );
  }
}

class _LightSectionDropdown extends StatelessWidget {
  final String? value;
  final List<_ExpertSectionOption> sections;
  final bool enabled;
  final ValueChanged<String?> onChanged;

  const _LightSectionDropdown({
    required this.value,
    required this.sections,
    this.enabled = true,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    final sectionIds = [
      for (final section in sections) section.id,
    ];
    if (value != null && !sectionIds.contains(value)) {
      sectionIds.add(value!);
    }
    return _CompactDropdown<String>(
      value: value ?? '__main__',
      items: [
        '__main__',
        ...sectionIds,
      ],
      enabled: enabled,
      itemLabel: (id) {
        if (id == '__main__') return 'Main';
        for (final section in sections) {
          if (section.id == id) return section.name;
        }
        return id;
      },
      onChanged: (id) => onChanged(id == '__main__' ? null : id),
    );
  }
}

class _CompactDropdown<T> extends StatelessWidget {
  final T value;
  final List<T> items;
  final String Function(T value) itemLabel;
  final bool enabled;
  final ValueChanged<T> onChanged;

  const _CompactDropdown({
    required this.value,
    required this.items,
    required this.itemLabel,
    this.enabled = true,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      constraints: const BoxConstraints(maxWidth: 150),
      padding: const EdgeInsets.symmetric(horizontal: 8),
      decoration: BoxDecoration(
        color: _panelColor,
        borderRadius: BorderRadius.circular(7),
        border: Border.all(color: CelestialColors.orbitRing),
      ),
      child: DropdownButtonHideUnderline(
        child: DropdownButton<T>(
          value: value,
          isExpanded: true,
          dropdownColor: _panel2Color,
          isDense: true,
          style: const TextStyle(
            color: CelestialColors.textPrimary,
            fontSize: 12,
            fontWeight: FontWeight.w600,
          ),
          selectedItemBuilder: (context) => [
            for (final item in items)
              Align(
                alignment: Alignment.centerLeft,
                child: Text(
                  itemLabel(item),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                ),
              ),
          ],
          items: [
            for (final item in items)
              DropdownMenuItem<T>(
                value: item,
                child: ConstrainedBox(
                  constraints: const BoxConstraints(maxWidth: 180),
                  child: Text(
                    itemLabel(item),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
              ),
          ],
          onChanged: enabled
              ? (value) {
                  if (value != null) onChanged(value);
                }
              : null,
        ),
      ),
    );
  }
}

class _OutdoorStatusPanel extends StatefulWidget {
  final _ExpertOutdoorStatus? status;
  final bool loading;
  final String? savingKey;
  final VoidCallback onRefresh;
  final VoidCallback onLearnBaselines;
  final void Function(String condition, int? durationMinutes) onSetOverride;
  final VoidCallback onClearOverride;

  const _OutdoorStatusPanel({
    required this.status,
    required this.loading,
    required this.savingKey,
    required this.onRefresh,
    required this.onLearnBaselines,
    required this.onSetOverride,
    required this.onClearOverride,
  });

  @override
  State<_OutdoorStatusPanel> createState() => _OutdoorStatusPanelState();
}

class _OutdoorStatusPanelState extends State<_OutdoorStatusPanel> {
  String _condition = 'cloudy';
  String _duration = '60';

  @override
  Widget build(BuildContext context) {
    final status = widget.status;
    final busy = widget.loading || widget.savingKey != null;
    final override = status?.override;
    final source = status == null ? '-' : _outdoorSourceLabel(status.source);

    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 9),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              const Expanded(
                child: Text(
                  'Sun intensity',
                  style: TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 13,
                    fontWeight: FontWeight.w800,
                  ),
                ),
              ),
              if (widget.loading)
                const SizedBox(
                  width: 16,
                  height: 16,
                  child: CircularProgressIndicator(strokeWidth: 2),
                ),
              IconButton(
                onPressed: busy ? null : widget.onRefresh,
                tooltip: 'Refresh sun intensity',
                icon: const Icon(Icons.refresh_rounded),
                iconSize: 18,
                color: CelestialColors.textSecondary,
                visualDensity: VisualDensity.compact,
              ),
            ],
          ),
          const SizedBox(height: 8),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [
              _AreaMetricChip(
                icon: Icons.wb_sunny_rounded,
                label: 'intensity',
                value: status == null
                    ? '-'
                    : '${(status.outdoorNormalized * 100).round()}%',
              ),
              _AreaMetricChip(
                icon: Icons.explore_rounded,
                label: 'sun',
                value: status == null
                    ? '-'
                    : '${(status.angleFactor * 100).round()}%',
              ),
              _AreaMetricChip(
                icon: Icons.cloud_rounded,
                label: 'clarity',
                value: status == null
                    ? '-'
                    : '${(status.conditionMultiplier * 100).round()}%',
              ),
              _AreaMetricChip(
                icon: Icons.sensors_rounded,
                label: 'source',
                value: source,
              ),
            ],
          ),
          if (status?.luxSmoothed != null ||
              status?.luxLearnedFloor != null ||
              status?.luxLearnedCeiling != null) ...[
            const SizedBox(height: 8),
            Text(
              [
                if (status?.luxSmoothed != null)
                  'Lux ${_compactNumber(status!.luxSmoothed!)}',
                if (status?.luxLearnedFloor != null)
                  'floor ${_compactNumber(status!.luxLearnedFloor!)}',
                if (status?.luxLearnedCeiling != null)
                  'ceiling ${_compactNumber(status!.luxLearnedCeiling!)}',
              ].join(' · '),
              style: const TextStyle(
                color: CelestialColors.textSecondary,
                fontSize: 11,
              ),
            ),
          ],
          if (override != null) ...[
            const SizedBox(height: 8),
            Text(
              'Override: ${_outdoorConditionLabel(override.condition, status)} · ${_outdoorOverrideDuration(override.expiresInMinutes)}',
              style: const TextStyle(
                color: _changedAccent,
                fontSize: 12,
                fontWeight: FontWeight.w700,
              ),
            ),
          ],
          const SizedBox(height: 10),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              _CompactTextButton(
                label: widget.savingKey == 'learn' ? 'Learning...' : 'Learn',
                onPressed: busy ? null : widget.onLearnBaselines,
              ),
              if (override != null)
                _CompactTextButton(
                  label: widget.savingKey == 'clear_override'
                      ? 'Clearing...'
                      : 'Clear',
                  color: _dangerAccent,
                  onPressed: busy ? null : widget.onClearOverride,
                ),
              _CompactDropdown<String>(
                value: _condition,
                items: _outdoorOverrideConditions,
                itemLabel: (value) => _outdoorConditionLabel(value, status),
                onChanged: (value) => setState(() => _condition = value),
              ),
              _CompactDropdown<String>(
                value: _duration,
                items: const ['15', '60', '240', '0'],
                itemLabel: _durationOptionLabel,
                onChanged: (value) => setState(() => _duration = value),
              ),
              _CompactTextButton(
                label: widget.savingKey == 'override' ? 'Setting...' : 'Set',
                onPressed: busy
                    ? null
                    : () => widget.onSetOverride(
                          _condition,
                          _duration == '0' ? null : int.parse(_duration),
                        ),
              ),
            ],
          ),
        ],
      ),
    );
  }
}

class _ConfigSectionCard extends StatelessWidget {
  final String title;
  final List<Widget> children;

  const _ConfigSectionCard({
    required this.title,
    required this.children,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(top: 12),
      child: _ExpertPanel(
        padding: const EdgeInsets.fromLTRB(14, 12, 14, 8),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              title,
              style: const TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 13,
                fontWeight: FontWeight.w800,
              ),
            ),
            const SizedBox(height: 6),
            for (int i = 0; i < children.length; i++) ...[
              children[i],
              if (i != children.length - 1)
                Divider(
                  height: 1,
                  color: CelestialColors.orbitRing.withValues(alpha: 0.55),
                ),
            ],
          ],
        ),
      ),
    );
  }
}

class _ConfigNumberRow extends StatelessWidget {
  final String label;
  final String unit;
  final num value;
  final num min;
  final num max;
  final num step;
  final bool saving;
  final bool enabled;
  final ValueChanged<num> onChanged;

  const _ConfigNumberRow({
    required this.label,
    required this.unit,
    required this.value,
    required this.min,
    required this.max,
    required this.step,
    required this.saving,
    this.enabled = true,
    required this.onChanged,
  });

  bool get _integerMode => value is int && step.toDouble() % 1 == 0;

  void _stepBy(num delta) {
    final next = _steppedConfigNumber(
      value + delta,
      min: min,
      max: max,
      step: step,
      integer: _integerMode,
    );
    if (next == value) return;
    onChanged(next);
  }

  @override
  Widget build(BuildContext context) {
    final atMin = value <= min;
    final atMax = value >= max;
    final valueText = '${_formatConfigNumber(value)} $unit';
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 9),
      child: Row(
        children: [
          Expanded(
            child: Text(
              label,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                color: enabled
                    ? CelestialColors.textPrimary
                    : CelestialColors.textSecondary,
                fontSize: 13,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
          const SizedBox(width: 10),
          if (saving) ...[
            const SizedBox(
              width: 14,
              height: 14,
              child: CircularProgressIndicator(strokeWidth: 2),
            ),
            const SizedBox(width: 8),
          ],
          _SmallSquareButton(
            icon: Icons.remove_rounded,
            onPressed:
                !enabled || saving || atMin ? null : () => _stepBy(-step),
          ),
          Container(
            width: 86,
            alignment: Alignment.center,
            padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 5),
            margin: const EdgeInsets.symmetric(horizontal: 6),
            decoration: BoxDecoration(
              color: _panelColor,
              borderRadius: BorderRadius.circular(7),
              border: Border.all(color: CelestialColors.orbitRing),
            ),
            child: Text(
              valueText,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: const TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 12,
                fontWeight: FontWeight.w800,
              ),
            ),
          ),
          _SmallSquareButton(
            icon: Icons.add_rounded,
            onPressed: !enabled || saving || atMax ? null : () => _stepBy(step),
          ),
        ],
      ),
    );
  }
}

class _ConfigDropdownRow<T> extends StatelessWidget {
  final String label;
  final T value;
  final List<T> items;
  final String Function(T value) itemLabel;
  final bool saving;
  final bool enabled;
  final ValueChanged<T> onChanged;

  const _ConfigDropdownRow({
    required this.label,
    required this.value,
    required this.items,
    required this.itemLabel,
    required this.saving,
    this.enabled = true,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    final menuItems = items.contains(value) ? items : [value, ...items];
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 9),
      child: Row(
        children: [
          Expanded(
            child: Text(
              label,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                color: enabled
                    ? CelestialColors.textPrimary
                    : CelestialColors.textSecondary,
                fontSize: 13,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
          const SizedBox(width: 10),
          if (saving) ...[
            const SizedBox(
              width: 14,
              height: 14,
              child: CircularProgressIndicator(strokeWidth: 2),
            ),
            const SizedBox(width: 8),
          ],
          ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 190, minWidth: 128),
            child: Container(
              padding: const EdgeInsets.symmetric(horizontal: 9),
              decoration: BoxDecoration(
                color: _panelColor,
                borderRadius: BorderRadius.circular(7),
                border: Border.all(color: CelestialColors.orbitRing),
              ),
              child: DropdownButtonHideUnderline(
                child: DropdownButton<T>(
                  value: value,
                  isExpanded: true,
                  dropdownColor: _panel2Color,
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 12,
                    fontWeight: FontWeight.w700,
                  ),
                  selectedItemBuilder: (context) => [
                    for (final item in menuItems)
                      Align(
                        alignment: Alignment.centerLeft,
                        child: Text(
                          itemLabel(item),
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                        ),
                      ),
                  ],
                  items: [
                    for (final item in menuItems)
                      DropdownMenuItem<T>(
                        value: item,
                        child: Text(
                          itemLabel(item),
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                        ),
                      ),
                  ],
                  onChanged: !enabled || saving
                      ? null
                      : (value) {
                          if (value != null) onChanged(value);
                        },
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _ConfigToggleRow extends StatelessWidget {
  final String label;
  final bool value;
  final bool saving;
  final bool enabled;
  final ValueChanged<bool> onChanged;

  const _ConfigToggleRow({
    required this.label,
    required this.value,
    required this.saving,
    this.enabled = true,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 6),
      child: Row(
        children: [
          Expanded(
            child: Text(
              label,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                color: enabled
                    ? CelestialColors.textPrimary
                    : CelestialColors.textSecondary,
                fontSize: 13,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
          const SizedBox(width: 10),
          if (saving) ...[
            const SizedBox(
              width: 14,
              height: 14,
              child: CircularProgressIndicator(strokeWidth: 2),
            ),
            const SizedBox(width: 8),
          ],
          Switch(
            value: value,
            activeThumbColor: _expertAccent,
            activeTrackColor: _expertAccent.withValues(alpha: 0.35),
            inactiveThumbColor: CelestialColors.textSecondary,
            inactiveTrackColor: _panel2Color,
            onChanged: !enabled || saving ? null : onChanged,
          ),
        ],
      ),
    );
  }
}

class _ConfigTimeRow extends StatelessWidget {
  final String label;
  final int hour;
  final int minute;
  final bool saving;
  final VoidCallback onTap;

  const _ConfigTimeRow({
    required this.label,
    required this.hour,
    required this.minute,
    required this.saving,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 9),
      child: Row(
        children: [
          Expanded(
            child: Text(
              label,
              style: const TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 13,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
          if (saving) ...[
            const SizedBox(
              width: 14,
              height: 14,
              child: CircularProgressIndicator(strokeWidth: 2),
            ),
            const SizedBox(width: 8),
          ],
          OutlinedButton.icon(
            onPressed: saving ? null : onTap,
            icon: const Icon(Icons.schedule_rounded, size: 16),
            label: Text(_formatConfigTime(hour, minute)),
            style: OutlinedButton.styleFrom(
              foregroundColor: CelestialColors.textPrimary,
              side: const BorderSide(color: CelestialColors.orbitRing),
              shape: RoundedRectangleBorder(
                borderRadius: BorderRadius.circular(7),
              ),
              minimumSize: const Size(104, 34),
              padding: const EdgeInsets.symmetric(horizontal: 10),
            ),
          ),
        ],
      ),
    );
  }
}

class _ExpertZoneNavRow extends StatelessWidget {
  final _ExpertZone zone;
  final bool connected;
  final Set<String> areaBusyIds;
  final bool Function(_ExpertArea area) areaPowerValue;
  final double Function(_ExpertArea area) areaBrightnessValue;
  final void Function(_ExpertArea area) onAreaPowerToggle;
  final void Function(_ExpertArea area, double brightness)
      onAreaBrightnessChanged;
  final void Function(_ExpertArea area, double brightness)
      onAreaBrightnessChangeEnd;
  final void Function(_ExpertArea area, String direction) onAreaBrightnessStep;
  final VoidCallback onLive;
  final VoidCallback onDefine;

  const _ExpertZoneNavRow({
    required this.zone,
    required this.connected,
    required this.areaBusyIds,
    required this.areaPowerValue,
    required this.areaBrightnessValue,
    required this.onAreaPowerToggle,
    required this.onAreaBrightnessChanged,
    required this.onAreaBrightnessChangeEnd,
    required this.onAreaBrightnessStep,
    required this.onLive,
    required this.onDefine,
  });

  @override
  Widget build(BuildContext context) {
    final state = zone.currentState;
    return _ExpertPanel(
      padding: const EdgeInsets.fromLTRB(14, 12, 14, 14),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Container(
            padding: const EdgeInsets.fromLTRB(10, 8, 8, 8),
            decoration: BoxDecoration(
              color: _panel2Color.withValues(alpha: 0.82),
              borderRadius: BorderRadius.circular(8),
              border: Border.all(
                color: CelestialColors.orbitRing.withValues(alpha: 0.65),
              ),
            ),
            child: Row(
              children: [
                Expanded(
                  child: GestureDetector(
                    onTap: onLive,
                    behavior: HitTestBehavior.opaque,
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Row(
                          children: [
                            Flexible(
                              child: Text(
                                zone.name,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: const TextStyle(
                                  color: CelestialColors.textPrimary,
                                  fontSize: 15,
                                  fontWeight: FontWeight.w800,
                                ),
                              ),
                            ),
                            if (zone.isDefault) ...[
                              const SizedBox(width: 8),
                              const _DefaultBadge(),
                            ],
                          ],
                        ),
                        const SizedBox(height: 4),
                        Row(
                          children: [
                            if (state != null) ...[
                              Container(
                                width: 9,
                                height: 9,
                                decoration: BoxDecoration(
                                  color: _cctToColor(state.kelvin),
                                  shape: BoxShape.circle,
                                ),
                              ),
                              const SizedBox(width: 6),
                            ],
                            Expanded(
                              child: Text(
                                [
                                  _areaCountLabel(zone.areas.length),
                                  if (state != null) state.stateLabel,
                                ].join(' · '),
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: const TextStyle(
                                  color: CelestialColors.textSecondary,
                                  fontSize: 11,
                                  fontFeatures: [FontFeature.tabularFigures()],
                                ),
                              ),
                            ),
                          ],
                        ),
                      ],
                    ),
                  ),
                ),
                if (state != null) ...[
                  const SizedBox(width: 10),
                  Text(
                    state.targetLabel,
                    style: const TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 13,
                      fontWeight: FontWeight.w800,
                      fontFeatures: [FontFeature.tabularFigures()],
                    ),
                  ),
                ],
                IconButton(
                  onPressed: onDefine,
                  tooltip: 'Define rhythm',
                  icon: const Icon(Icons.tune_rounded, size: 18),
                  color: CelestialColors.textSecondary,
                  visualDensity: VisualDensity.compact,
                ),
              ],
            ),
          ),
          if (zone.areas.isNotEmpty) ...[
            const SizedBox(height: 4),
            for (final area in zone.areas)
              _ExpertMainAreaControlRow(
                area: area,
                connected: connected,
                busy: areaBusyIds.contains(area.id),
                powerOn: areaPowerValue(area),
                brightness: areaBrightnessValue(area),
                onPowerToggle: () => onAreaPowerToggle(area),
                onBrightnessChanged: (value) =>
                    onAreaBrightnessChanged(area, value),
                onBrightnessChangeEnd: (value) =>
                    onAreaBrightnessChangeEnd(area, value),
                onBrightnessStepDown: () => onAreaBrightnessStep(area, 'down'),
                onBrightnessStepUp: () => onAreaBrightnessStep(area, 'up'),
              ),
          ],
        ],
      ),
    );
  }
}

class _ExpertMainAreaControlRow extends StatelessWidget {
  final _ExpertArea area;
  final bool connected;
  final bool busy;
  final bool powerOn;
  final double brightness;
  final VoidCallback onPowerToggle;
  final ValueChanged<double> onBrightnessChanged;
  final ValueChanged<double> onBrightnessChangeEnd;
  final VoidCallback onBrightnessStepDown;
  final VoidCallback onBrightnessStepUp;

  const _ExpertMainAreaControlRow({
    required this.area,
    required this.connected,
    required this.busy,
    required this.powerOn,
    required this.brightness,
    required this.onPowerToggle,
    required this.onBrightnessChanged,
    required this.onBrightnessChangeEnd,
    required this.onBrightnessStepDown,
    required this.onBrightnessStepUp,
  });

  @override
  Widget build(BuildContext context) {
    final enabled = connected && !busy && !area.stale;
    final safeBrightness = brightness.clamp(1, 100).toDouble();
    final accent = powerOn
        ? _expertAccent
        : CelestialColors.textSecondary.withValues(alpha: 0.65);
    return Container(
      margin: const EdgeInsets.only(top: 8),
      padding: const EdgeInsets.fromLTRB(10, 8, 10, 8),
      decoration: BoxDecoration(
        color: _panel2Color.withValues(alpha: 0.72),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(
          color: CelestialColors.orbitRing.withValues(alpha: 0.65),
        ),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Tooltip(
                message: powerOn ? 'Turn room off' : 'Turn room on',
                child: IconButton(
                  onPressed: enabled ? onPowerToggle : null,
                  icon: Icon(
                    powerOn
                        ? Icons.power_settings_new_rounded
                        : Icons.power_off_rounded,
                    size: 20,
                  ),
                  color: accent,
                  visualDensity: VisualDensity.compact,
                  style: IconButton.styleFrom(
                    fixedSize: const Size(36, 36),
                    padding: EdgeInsets.zero,
                    backgroundColor: accent.withValues(alpha: 0.12),
                  ),
                ),
              ),
              const SizedBox(width: 10),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      area.name,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(
                        color: CelestialColors.textPrimary,
                        fontSize: 13,
                        fontWeight: FontWeight.w800,
                      ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      area.stale
                          ? '${area.deviceCount} lights - removed'
                          : '${area.deviceCount} lights',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        color: area.stale
                            ? _dangerAccent
                            : CelestialColors.textSecondary,
                        fontSize: 10,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                  ],
                ),
              ),
              const SizedBox(width: 10),
              AnimatedSwitcher(
                duration: const Duration(milliseconds: 140),
                child: busy
                    ? const SizedBox(
                        key: ValueKey('busy'),
                        width: 18,
                        height: 18,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      )
                    : Text(
                        powerOn ? '${safeBrightness.round()}%' : 'Off',
                        key: ValueKey(
                          '${area.id}:$powerOn:${safeBrightness.round()}',
                        ),
                        style: const TextStyle(
                          color: CelestialColors.textPrimary,
                          fontSize: 13,
                          fontWeight: FontWeight.w800,
                          fontFeatures: [FontFeature.tabularFigures()],
                        ),
                      ),
              ),
              const SizedBox(width: 6),
              _ExpertRowNudgeButton(
                icon: Icons.remove_rounded,
                onPressed: enabled ? onBrightnessStepDown : null,
                tooltip: 'Dimmer',
              ),
              const SizedBox(width: 4),
              _ExpertRowNudgeButton(
                icon: Icons.add_rounded,
                onPressed: enabled ? onBrightnessStepUp : null,
                tooltip: 'Brighter',
              ),
            ],
          ),
          Row(
            children: [
              const SizedBox(width: 46),
              Expanded(
                child: SliderTheme(
                  data: SliderTheme.of(context).copyWith(
                    activeTrackColor: accent,
                    inactiveTrackColor:
                        CelestialColors.orbitRing.withValues(alpha: 0.7),
                    thumbColor: accent,
                    overlayColor: accent.withValues(alpha: 0.12),
                    trackHeight: 3,
                  ),
                  child: Slider(
                    min: 1,
                    max: 100,
                    divisions: 99,
                    value: safeBrightness,
                    onChanged: enabled ? onBrightnessChanged : null,
                    onChangeEnd: enabled ? onBrightnessChangeEnd : null,
                  ),
                ),
              ),
            ],
          ),
        ],
      ),
    );
  }
}

class _ExpertRowNudgeButton extends StatelessWidget {
  final IconData icon;
  final VoidCallback? onPressed;
  final String tooltip;

  const _ExpertRowNudgeButton({
    required this.icon,
    required this.onPressed,
    required this.tooltip,
  });

  @override
  Widget build(BuildContext context) {
    return Tooltip(
      message: tooltip,
      child: IconButton(
        onPressed: onPressed,
        icon: Icon(icon, size: 16),
        color: CelestialColors.textPrimary,
        disabledColor: CelestialColors.textSecondary.withValues(alpha: 0.35),
        visualDensity: VisualDensity.compact,
        style: IconButton.styleFrom(
          fixedSize: const Size(28, 28),
          padding: EdgeInsets.zero,
          backgroundColor: Colors.black.withValues(alpha: 0.22),
          side: BorderSide(
            color: CelestialColors.orbitRing.withValues(alpha: 0.65),
          ),
        ),
      ),
    );
  }
}

class _ExpertServerStatus extends StatelessWidget {
  final String status;
  final bool connected;
  final bool loading;
  final _ExpertServerSnapshot? snapshot;
  final Future<void> Function() onRefresh;

  const _ExpertServerStatus({
    required this.status,
    required this.connected,
    required this.loading,
    required this.snapshot,
    required this.onRefresh,
  });

  @override
  Widget build(BuildContext context) {
    final color = connected ? const Color(0xFF22C55E) : _expertAccent;
    return Container(
      padding: const EdgeInsets.fromLTRB(12, 10, 8, 10),
      decoration: BoxDecoration(
        color: _panelColor,
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: CelestialColors.orbitRing),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              if (loading)
                const SizedBox(
                  width: 12,
                  height: 12,
                  child: CircularProgressIndicator(strokeWidth: 2),
                )
              else
                Container(
                  width: 9,
                  height: 9,
                  decoration: BoxDecoration(
                    color: color,
                    shape: BoxShape.circle,
                  ),
                ),
              const SizedBox(width: 10),
              Expanded(
                child: Text(
                  status,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color:
                        CelestialColors.textSecondary.withValues(alpha: 0.92),
                    fontSize: 12,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
              if (snapshot?.channelLabel != null) ...[
                const SizedBox(width: 8),
                _ExpertChannelBadge(label: snapshot!.channelLabel!),
              ],
              IconButton(
                onPressed: loading ? null : onRefresh,
                tooltip: 'Refresh expert zones',
                icon: const Icon(Icons.refresh_rounded),
                color: CelestialColors.textSecondary,
                visualDensity: VisualDensity.compact,
              ),
            ],
          ),
          if (connected && snapshot != null && snapshot!.hasMetrics) ...[
            const SizedBox(height: 8),
            Wrap(
              spacing: 8,
              runSpacing: 8,
              children: [
                if (snapshot!.serverHour != null)
                  _AreaMetricChip(
                    icon: Icons.schedule_rounded,
                    label: snapshot!.timezoneLabel,
                    value: _formatHour(snapshot!.serverHour!),
                  ),
                if (snapshot!.brightness != null && snapshot!.kelvin != null)
                  _AreaMetricChip(
                    icon: Icons.light_mode_rounded,
                    label: 'now',
                    value: '${snapshot!.brightness}% / ${snapshot!.kelvin} K',
                  ),
                if (snapshot!.locationLabel != null)
                  _AreaMetricChip(
                    icon: Icons.explore_rounded,
                    label: 'location',
                    value: snapshot!.locationLabel!,
                  ),
              ],
            ),
          ],
        ],
      ),
    );
  }
}

class _ExpertChannelBadge extends StatelessWidget {
  final String label;

  const _ExpertChannelBadge({required this.label});

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 4),
      decoration: BoxDecoration(
        color: _expertAccent.withValues(alpha: 0.14),
        borderRadius: BorderRadius.circular(6),
        border: Border.all(color: _expertAccent.withValues(alpha: 0.55)),
      ),
      child: Text(
        label,
        style: const TextStyle(
          color: _expertAccent,
          fontSize: 10,
          fontWeight: FontWeight.w900,
          letterSpacing: 0,
        ),
      ),
    );
  }
}

class _ExpertToolStrip extends StatelessWidget {
  final VoidCallback onActivity;
  final VoidCallback onMoments;
  final VoidCallback onSwitches;
  final VoidCallback onSettings;

  const _ExpertToolStrip({
    required this.onActivity,
    required this.onMoments,
    required this.onSwitches,
    required this.onSettings,
  });

  @override
  Widget build(BuildContext context) {
    return Column(
      children: [
        Row(
          children: [
            Expanded(
              child: _ExpertToolButton(
                icon: Icons.history_rounded,
                label: 'Activity',
                onTap: onActivity,
              ),
            ),
            const SizedBox(width: 8),
            Expanded(
              child: _ExpertToolButton(
                icon: Icons.auto_awesome_rounded,
                label: 'Moments',
                onTap: onMoments,
              ),
            ),
          ],
        ),
        const SizedBox(height: 8),
        Row(
          children: [
            Expanded(
              child: _ExpertToolButton(
                icon: Icons.sensors_rounded,
                label: 'Controls',
                onTap: onSwitches,
              ),
            ),
            const SizedBox(width: 8),
            Expanded(
              child: _ExpertToolButton(
                icon: Icons.settings_rounded,
                label: 'Settings',
                onTap: onSettings,
              ),
            ),
          ],
        ),
      ],
    );
  }
}

class _ExpertToolButton extends StatelessWidget {
  final IconData icon;
  final String label;
  final VoidCallback onTap;

  const _ExpertToolButton({
    required this.icon,
    required this.label,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    return OutlinedButton.icon(
      onPressed: onTap,
      icon: Icon(icon, size: 17),
      label: Text(label),
      style: OutlinedButton.styleFrom(
        foregroundColor: CelestialColors.textPrimary,
        side: const BorderSide(color: CelestialColors.orbitRing),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
        padding: const EdgeInsets.symmetric(vertical: 12, horizontal: 10),
      ),
    );
  }
}

class _ExpertZoneManageRow extends StatelessWidget {
  final _ExpertZone zone;
  final bool canMoveUp;
  final bool canMoveDown;
  final bool canDelete;
  final VoidCallback onMoveUp;
  final VoidCallback onMoveDown;
  final VoidCallback onRename;
  final VoidCallback onMakeDefault;
  final VoidCallback onDelete;

  const _ExpertZoneManageRow({
    required this.zone,
    required this.canMoveUp,
    required this.canMoveDown,
    required this.canDelete,
    required this.onMoveUp,
    required this.onMoveDown,
    required this.onRename,
    required this.onMakeDefault,
    required this.onDelete,
  });

  @override
  Widget build(BuildContext context) {
    return _ExpertPanel(
      padding: const EdgeInsets.fromLTRB(10, 12, 12, 12),
      child: Row(
        children: [
          Column(
            children: [
              _SmallSquareButton(
                icon: Icons.keyboard_arrow_up_rounded,
                onPressed: canMoveUp ? onMoveUp : null,
              ),
              const SizedBox(height: 4),
              _SmallSquareButton(
                icon: Icons.keyboard_arrow_down_rounded,
                onPressed: canMoveDown ? onMoveDown : null,
              ),
            ],
          ),
          const SizedBox(width: 12),
          Expanded(
            child: GestureDetector(
              onTap: onRename,
              behavior: HitTestBehavior.opaque,
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      Flexible(
                        child: Text(
                          zone.name,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: const TextStyle(
                            color: CelestialColors.textPrimary,
                            fontSize: 15,
                            fontWeight: FontWeight.w700,
                          ),
                        ),
                      ),
                      const SizedBox(width: 8),
                      Icon(
                        Icons.edit_rounded,
                        size: 14,
                        color: CelestialColors.textSecondary
                            .withValues(alpha: 0.7),
                      ),
                    ],
                  ),
                  const SizedBox(height: 4),
                  Text(
                    zone.isDefault
                        ? '${_areaCountLabel(zone.areas.length)} / default'
                        : _areaCountLabel(zone.areas.length),
                    style: const TextStyle(
                      color: CelestialColors.textSecondary,
                      fontSize: 11,
                    ),
                  ),
                ],
              ),
            ),
          ),
          if (!zone.isDefault)
            _CompactTextButton(label: 'Default', onPressed: onMakeDefault),
          const SizedBox(width: 6),
          _CompactTextButton(
            label: 'Delete',
            color: _dangerAccent,
            onPressed: canDelete ? onDelete : null,
          ),
        ],
      ),
    );
  }
}

class _ExpertZoneAreaManageList extends StatelessWidget {
  final _ExpertZone zone;
  final List<String> zoneNames;
  final ValueChanged<int> onMoveUp;
  final ValueChanged<int> onMoveDown;
  final void Function(int areaIndex, String targetZoneName) onMoveToZone;
  final ValueChanged<int> onRemoveFromZone;
  final ValueChanged<int> onPurgeStaleArea;

  const _ExpertZoneAreaManageList({
    required this.zone,
    required this.zoneNames,
    required this.onMoveUp,
    required this.onMoveDown,
    required this.onMoveToZone,
    required this.onRemoveFromZone,
    required this.onPurgeStaleArea,
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      margin: const EdgeInsets.only(top: 6),
      decoration: BoxDecoration(
        color: _panelColor.withValues(alpha: 0.64),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(
          color: CelestialColors.orbitRing.withValues(alpha: 0.72),
        ),
      ),
      clipBehavior: Clip.antiAlias,
      child: Column(
        children: [
          if (zone.areas.isEmpty)
            Padding(
              padding: const EdgeInsets.fromLTRB(12, 10, 12, 10),
              child: Row(
                children: const [
                  Icon(
                    Icons.meeting_room_outlined,
                    color: CelestialColors.textSecondary,
                    size: 16,
                  ),
                  SizedBox(width: 8),
                  Expanded(
                    child: Text(
                      'No areas',
                      style: TextStyle(
                        color: CelestialColors.textSecondary,
                        fontSize: 12,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                  ),
                ],
              ),
            )
          else
            for (var i = 0; i < zone.areas.length; i++)
              _ExpertZoneAreaManageRow(
                area: zone.areas[i],
                zoneName: zone.name,
                zoneNames: zoneNames,
                canMoveUp: i > 0,
                canMoveDown: i < zone.areas.length - 1,
                showDivider: i < zone.areas.length - 1,
                onMoveUp: () => onMoveUp(i),
                onMoveDown: () => onMoveDown(i),
                onMoveToZone: (targetZoneName) =>
                    onMoveToZone(i, targetZoneName),
                onRemoveFromZone:
                    zone.isDefault ? null : () => onRemoveFromZone(i),
                onPurgeStaleArea:
                    zone.areas[i].stale ? () => onPurgeStaleArea(i) : null,
              ),
        ],
      ),
    );
  }
}

class _ExpertZoneAreaManageRow extends StatelessWidget {
  final _ExpertArea area;
  final String zoneName;
  final List<String> zoneNames;
  final bool canMoveUp;
  final bool canMoveDown;
  final bool showDivider;
  final VoidCallback onMoveUp;
  final VoidCallback onMoveDown;
  final ValueChanged<String> onMoveToZone;
  final VoidCallback? onRemoveFromZone;
  final VoidCallback? onPurgeStaleArea;

  const _ExpertZoneAreaManageRow({
    required this.area,
    required this.zoneName,
    required this.zoneNames,
    required this.canMoveUp,
    required this.canMoveDown,
    required this.showDivider,
    required this.onMoveUp,
    required this.onMoveDown,
    required this.onMoveToZone,
    required this.onRemoveFromZone,
    required this.onPurgeStaleArea,
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.fromLTRB(8, 8, 8, 8),
      decoration: BoxDecoration(
        border: showDivider
            ? Border(
                bottom: BorderSide(
                  color: CelestialColors.orbitRing.withValues(alpha: 0.55),
                ),
              )
            : null,
      ),
      child: Row(
        children: [
          Column(
            children: [
              _SmallSquareButton(
                icon: Icons.keyboard_arrow_up_rounded,
                onPressed: canMoveUp ? onMoveUp : null,
              ),
              const SizedBox(height: 4),
              _SmallSquareButton(
                icon: Icons.keyboard_arrow_down_rounded,
                onPressed: canMoveDown ? onMoveDown : null,
              ),
            ],
          ),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  area.name,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 13,
                    fontWeight: FontWeight.w800,
                  ),
                ),
                const SizedBox(height: 2),
                Text(
                  area.stale
                      ? '${area.deviceCount} lights - removed from Home Assistant'
                      : '${area.deviceCount} lights',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: area.stale
                        ? _dangerAccent
                        : CelestialColors.textSecondary,
                    fontSize: 10,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(width: 8),
          _AreaMoveMenu(
            zoneName: zoneName,
            zoneNames: zoneNames,
            onMoveToZone: onMoveToZone,
          ),
          if (onRemoveFromZone != null) ...[
            const SizedBox(width: 6),
            IconButton(
              onPressed: onRemoveFromZone,
              tooltip: 'Move to default zone',
              icon: const Icon(Icons.remove_circle_outline_rounded),
              iconSize: 18,
              style: IconButton.styleFrom(
                foregroundColor: _dangerAccent,
                backgroundColor: _panel2Color,
                fixedSize: const Size(34, 34),
                minimumSize: const Size(34, 34),
                padding: EdgeInsets.zero,
                shape: RoundedRectangleBorder(
                  borderRadius: BorderRadius.circular(7),
                ),
                side: const BorderSide(color: CelestialColors.orbitRing),
              ),
            ),
          ],
          if (onPurgeStaleArea != null) ...[
            const SizedBox(width: 6),
            IconButton(
              onPressed: onPurgeStaleArea,
              tooltip: 'Remove stale area',
              icon: const Icon(Icons.delete_sweep_rounded),
              iconSize: 18,
              style: IconButton.styleFrom(
                foregroundColor: _dangerAccent,
                backgroundColor: _panel2Color,
                fixedSize: const Size(34, 34),
                minimumSize: const Size(34, 34),
                padding: EdgeInsets.zero,
                shape: RoundedRectangleBorder(
                  borderRadius: BorderRadius.circular(7),
                ),
                side: const BorderSide(color: CelestialColors.orbitRing),
              ),
            ),
          ],
        ],
      ),
    );
  }
}

class _AreaMoveMenu extends StatelessWidget {
  final String zoneName;
  final List<String> zoneNames;
  final ValueChanged<String> onMoveToZone;

  const _AreaMoveMenu({
    required this.zoneName,
    required this.zoneNames,
    required this.onMoveToZone,
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      width: 82,
      height: 34,
      padding: const EdgeInsets.only(left: 10, right: 4),
      decoration: BoxDecoration(
        color: _panel2Color,
        borderRadius: BorderRadius.circular(7),
        border: Border.all(color: CelestialColors.orbitRing),
      ),
      child: DropdownButtonHideUnderline(
        child: DropdownButton<String>(
          value: zoneName,
          isExpanded: true,
          dropdownColor: _panel2Color,
          icon: const Icon(
            Icons.keyboard_arrow_down_rounded,
            color: CelestialColors.textSecondary,
            size: 18,
          ),
          selectedItemBuilder: (context) => [
            for (final _ in zoneNames)
              const Align(
                alignment: Alignment.centerLeft,
                child: Text(
                  'Move',
                  style: TextStyle(
                    color: CelestialColors.textSecondary,
                    fontSize: 12,
                    fontWeight: FontWeight.w700,
                  ),
                ),
              ),
          ],
          items: [
            for (final name in zoneNames)
              DropdownMenuItem(
                value: name,
                child: ConstrainedBox(
                  constraints: const BoxConstraints(maxWidth: 190),
                  child: Text(
                    name,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
              ),
          ],
          onChanged: (targetZoneName) {
            if (targetZoneName == null || targetZoneName == zoneName) return;
            onMoveToZone(targetZoneName);
          },
        ),
      ),
    );
  }
}

class _ZoneIdentityRow extends StatelessWidget {
  final String name;
  final String pattern;
  final List<String> patternOptions;
  final bool dirty;
  final ValueChanged<String> onPatternChanged;
  final VoidCallback onLive;

  const _ZoneIdentityRow({
    required this.name,
    required this.pattern,
    required this.patternOptions,
    required this.dirty,
    required this.onPatternChanged,
    required this.onLive,
  });

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        final compact = constraints.maxWidth < 440;
        final patternPill = _PatternPill(
          value: pattern,
          options: patternOptions,
          dirty: dirty,
          onChanged: onPatternChanged,
        );
        final title = Text(
          name,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: const TextStyle(
            color: CelestialColors.textPrimary,
            fontSize: 23,
            fontWeight: FontWeight.w800,
          ),
        );
        final controls = compact
            ? Row(
                children: [
                  Expanded(child: patternPill),
                  const SizedBox(width: 8),
                  _ZoneLinkButton(label: 'Live', onTap: onLive),
                ],
              )
            : Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  SizedBox(width: 150, child: patternPill),
                  const SizedBox(width: 8),
                  _ZoneLinkButton(label: 'Live', onTap: onLive),
                ],
              );

        if (compact) {
          return Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              title,
              const SizedBox(height: 8),
              controls,
            ],
          );
        }

        return Row(
          children: [
            Expanded(child: title),
            const SizedBox(width: 8),
            controls,
          ],
        );
      },
    );
  }
}

class _PatternPill extends StatelessWidget {
  final String value;
  final List<String> options;
  final bool dirty;
  final ValueChanged<String> onChanged;

  const _PatternPill({
    required this.value,
    required this.options,
    required this.dirty,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    final items = _stringOptionsWithValue(options, value);
    return Container(
      height: 32,
      padding: const EdgeInsets.only(left: 10, right: 4),
      decoration: BoxDecoration(
        color: Colors.white.withValues(alpha: 0.05),
        borderRadius: BorderRadius.circular(999),
        border: Border.all(
          color: dirty ? _changedAccent : CelestialColors.orbitRing,
        ),
      ),
      child: DropdownButtonHideUnderline(
        child: DropdownButton<String>(
          value: value,
          isExpanded: true,
          dropdownColor: _panel2Color,
          icon: Icon(
            Icons.keyboard_arrow_down_rounded,
            color: dirty ? _changedAccent : CelestialColors.textSecondary,
            size: 18,
          ),
          style: TextStyle(
            color: dirty ? _changedAccent : CelestialColors.textSecondary,
            fontSize: 12,
            fontWeight: FontWeight.w600,
          ),
          selectedItemBuilder: (context) => [
            for (final option in items)
              Align(
                alignment: Alignment.centerLeft,
                child: Text(
                  _patternLabel(option),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                ),
              ),
          ],
          items: [
            for (final option in items)
              DropdownMenuItem(
                value: option,
                child: Text(_patternLabel(option)),
              ),
          ],
          onChanged: (value) {
            if (value != null) onChanged(value);
          },
        ),
      ),
    );
  }
}

class _TuneSectionCard extends StatelessWidget {
  final String id;
  final String title;
  final String value;
  final bool expanded;
  final VoidCallback onToggle;
  final Widget child;

  const _TuneSectionCard({
    required this.id,
    required this.title,
    required this.value,
    required this.expanded,
    required this.onToggle,
    required this.child,
  });

  @override
  Widget build(BuildContext context) {
    return AnimatedContainer(
      duration: const Duration(milliseconds: 220),
      margin: const EdgeInsets.only(bottom: 12),
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(10),
        border: Border.all(
          color: expanded
              ? _expertAccent.withValues(alpha: 0.45)
              : CelestialColors.orbitRing,
        ),
      ),
      clipBehavior: Clip.antiAlias,
      child: Column(
        children: [
          GestureDetector(
            onTap: onToggle,
            behavior: HitTestBehavior.opaque,
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
              child: Row(
                children: [
                  AnimatedRotation(
                    turns: expanded ? 0.25 : 0,
                    duration: const Duration(milliseconds: 180),
                    child: const Icon(
                      Icons.chevron_right_rounded,
                      color: CelestialColors.textSecondary,
                    ),
                  ),
                  const SizedBox(width: 6),
                  Text(
                    title,
                    style: const TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 15,
                      fontWeight: FontWeight.w700,
                    ),
                  ),
                  const Spacer(),
                  if (!expanded)
                    Flexible(
                      child: Text(
                        value,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        textAlign: TextAlign.right,
                        style: const TextStyle(
                          color: CelestialColors.textSecondary,
                          fontSize: 13,
                          fontFeatures: [FontFeature.tabularFigures()],
                        ),
                      ),
                    ),
                ],
              ),
            ),
          ),
          ClipRect(
            child: AnimatedAlign(
              alignment: Alignment.topCenter,
              heightFactor: expanded ? 1 : 0,
              duration: const Duration(milliseconds: 220),
              curve: Curves.easeOutCubic,
              child: Padding(
                padding: const EdgeInsets.fromLTRB(14, 0, 14, 14),
                child: child,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _CurveCard extends StatelessWidget {
  final String title;
  final String subtitle;
  final _RhythmDefinition definition;
  final _ExpertCurveData? serverCurve;
  final double selectedHour;
  final Color accent;
  final ValueChanged<double> onHourChanged;

  const _CurveCard({
    required this.title,
    required this.subtitle,
    required this.definition,
    this.serverCurve,
    required this.selectedHour,
    required this.accent,
    required this.onHourChanged,
  });

  @override
  Widget build(BuildContext context) {
    return _ExpertPanel(
      padding: const EdgeInsets.fromLTRB(12, 12, 12, 10),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(
                  title,
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 18,
                    fontWeight: FontWeight.w800,
                    fontFeatures: [FontFeature.tabularFigures()],
                  ),
                ),
              ),
              Text(
                subtitle,
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.9),
                  fontSize: 12,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ],
          ),
          const SizedBox(height: 12),
          AspectRatio(
            aspectRatio: 1.72,
            child: LayoutBuilder(
              builder: (context, constraints) {
                return GestureDetector(
                  behavior: HitTestBehavior.opaque,
                  onTapDown: (details) => _selectHour(
                    details.localPosition.dx,
                    constraints.maxWidth,
                  ),
                  onHorizontalDragUpdate: (details) => _selectHour(
                    details.localPosition.dx,
                    constraints.maxWidth,
                  ),
                  child: CustomPaint(
                    painter: _CurvePainter(
                      definition: definition,
                      serverCurve: serverCurve,
                      selectedHour: selectedHour,
                      accent: accent,
                    ),
                  ),
                );
              },
            ),
          ),
        ],
      ),
    );
  }

  void _selectHour(double localX, double width) {
    final hour = (localX / width * 24).clamp(0, 24).toDouble();
    onHourChanged(hour);
  }
}

class _CurvePainter extends CustomPainter {
  final _RhythmDefinition definition;
  final _ExpertCurveData? serverCurve;
  final double selectedHour;
  final Color accent;

  const _CurvePainter({
    required this.definition,
    required this.serverCurve,
    required this.selectedHour,
    required this.accent,
  });

  @override
  void paint(Canvas canvas, Size size) {
    final chart = Rect.fromLTWH(0, 0, size.width, size.height - 22);
    final bgPaint = Paint()..color = const Color(0xFF0B0D12);
    canvas.drawRRect(
      RRect.fromRectAndRadius(chart, const Radius.circular(8)),
      bgPaint,
    );

    final gridPaint = Paint()
      ..color = CelestialColors.orbitRing.withValues(alpha: 0.55)
      ..strokeWidth = 0.7;
    for (int i = 0; i <= 6; i++) {
      final y = chart.top + chart.height * i / 6;
      canvas.drawLine(Offset(chart.left, y), Offset(chart.right, y), gridPaint);
    }
    for (int i = 0; i <= 6; i++) {
      final x = chart.left + chart.width * i / 6;
      canvas.drawLine(Offset(x, chart.top), Offset(x, chart.bottom), gridPaint);
    }

    final fillPath = Path()..moveTo(chart.left, chart.bottom);
    final linePath = Path();
    const samples = 144;
    for (int i = 0; i <= samples; i++) {
      final hour = i / samples * 24;
      final reading = _readingAt(hour);
      final x = chart.left + chart.width * hour / 24;
      final y = chart.bottom - chart.height * reading.brightness / 100;
      if (i == 0) {
        linePath.moveTo(x, y);
        fillPath.lineTo(x, y);
      } else {
        linePath.lineTo(x, y);
        fillPath.lineTo(x, y);
      }
    }
    fillPath
      ..lineTo(chart.right, chart.bottom)
      ..close();

    canvas.drawPath(
      fillPath,
      Paint()
        ..shader = LinearGradient(
          begin: Alignment.topCenter,
          end: Alignment.bottomCenter,
          colors: [
            accent.withValues(alpha: 0.28),
            accent.withValues(alpha: 0.03),
          ],
        ).createShader(chart),
    );

    final linePaint = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 2.4
      ..strokeCap = StrokeCap.round
      ..strokeJoin = StrokeJoin.round;
    final metrics = linePath.computeMetrics().toList(growable: false);
    if (metrics.isNotEmpty) {
      for (final metric in metrics) {
        final length = metric.length;
        double start = 0;
        while (start < length) {
          final end = math.min(start + 5, length);
          final segment = metric.extractPath(start, end);
          final progress = ((start + end) / 2 / length).clamp(0.0, 1.0);
          final hour = progress * 24;
          linePaint.color = _cctToColor(_readingAt(hour).kelvin);
          canvas.drawPath(segment, linePaint);
          start = end;
        }
      }
    }

    final selected = _readingAt(selectedHour);
    final cursorX = chart.left + chart.width * selectedHour / 24;
    final cursorY = chart.bottom - chart.height * selected.brightness / 100;
    final cursorPaint = Paint()
      ..color = accent
      ..strokeWidth = 1.4;
    canvas.drawLine(
      Offset(cursorX, chart.top),
      Offset(cursorX, chart.bottom),
      cursorPaint..color = accent.withValues(alpha: 0.75),
    );
    canvas.drawCircle(
      Offset(cursorX, cursorY),
      6,
      Paint()..color = const Color(0xFF0B0D12),
    );
    canvas.drawCircle(
      Offset(cursorX, cursorY),
      5,
      Paint()..color = accent,
    );

    final labelStyle = TextStyle(
      color: CelestialColors.textSecondary.withValues(alpha: 0.9),
      fontSize: 10,
      fontFeatures: const [FontFeature.tabularFigures()],
    );
    for (final hour in [0, 6, 12, 18, 24]) {
      final painter = TextPainter(
        text: TextSpan(text: _axisHour(hour), style: labelStyle),
        textDirection: TextDirection.ltr,
      )..layout();
      final x = chart.left + chart.width * hour / 24 - painter.width / 2;
      painter.paint(
        canvas,
        Offset(
            x.clamp(chart.left, chart.right - painter.width), chart.bottom + 7),
      );
    }
  }

  _CurveReading _readingAt(double hour) {
    return serverCurve?.readingAt(hour) ?? _CurveReading.from(definition, hour);
  }

  @override
  bool shouldRepaint(covariant _CurvePainter oldDelegate) {
    return oldDelegate.definition != definition ||
        oldDelegate.serverCurve != serverCurve ||
        oldDelegate.selectedHour != selectedHour ||
        oldDelegate.accent != accent;
  }
}

class _ExpertSliderRow extends StatelessWidget {
  final String label;
  final double value;
  final double min;
  final double max;
  final int divisions;
  final String display;
  final bool enabled;
  final ValueChanged<double> onChanged;

  const _ExpertSliderRow({
    required this.label,
    required this.value,
    required this.min,
    required this.max,
    required this.divisions,
    required this.display,
    this.enabled = true,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(top: 8),
      child: Column(
        children: [
          Row(
            children: [
              Expanded(
                child: Text(
                  label,
                  style: const TextStyle(
                    color: CelestialColors.textSecondary,
                    fontSize: 13,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
              Text(
                display,
                style: const TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 13,
                  fontWeight: FontWeight.w700,
                  fontFeatures: [FontFeature.tabularFigures()],
                ),
              ),
            ],
          ),
          SliderTheme(
            data: SliderTheme.of(context).copyWith(
              activeTrackColor: _expertAccent,
              inactiveTrackColor: CelestialColors.orbitRing,
              thumbColor: _expertAccent,
              overlayColor: _expertAccent.withValues(alpha: 0.14),
              trackHeight: 3,
            ),
            child: Slider(
              value: value.clamp(min, max),
              min: min,
              max: max,
              divisions: divisions,
              onChanged: enabled ? onChanged : null,
            ),
          ),
        ],
      ),
    );
  }
}

class _OptionRow extends StatelessWidget {
  final String label;
  final String value;
  final List<String> options;
  final String Function(String value) itemLabel;
  final bool enabled;
  final ValueChanged<String> onChanged;

  const _OptionRow({
    required this.label,
    required this.value,
    required this.options,
    this.itemLabel = _identityString,
    this.enabled = true,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    final items = _stringOptionsWithValue(options, value);
    return Padding(
      padding: const EdgeInsets.only(top: 8, bottom: 4),
      child: Row(
        children: [
          Expanded(
            child: Text(
              label,
              style: const TextStyle(
                color: CelestialColors.textSecondary,
                fontSize: 13,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
          SizedBox(
            width: 150,
            child: Container(
              padding: const EdgeInsets.symmetric(horizontal: 10),
              decoration: BoxDecoration(
                color: _panel2Color,
                borderRadius: BorderRadius.circular(7),
                border: Border.all(color: CelestialColors.orbitRing),
              ),
              child: DropdownButtonHideUnderline(
                child: DropdownButton<String>(
                  value: value,
                  isExpanded: true,
                  dropdownColor: _panel2Color,
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 13,
                  ),
                  selectedItemBuilder: (context) => [
                    for (final option in items)
                      Align(
                        alignment: Alignment.centerLeft,
                        child: Text(
                          itemLabel(option),
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                        ),
                      ),
                  ],
                  items: [
                    for (final option in items)
                      DropdownMenuItem(
                        value: option,
                        child: Text(itemLabel(option)),
                      ),
                  ],
                  onChanged: enabled
                      ? (value) {
                          if (value != null) onChanged(value);
                        }
                      : null,
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _AreaLiveRow extends StatelessWidget {
  final _ExpertArea area;
  final _CurveReading reading;
  final bool powerOn;
  final bool boosted;
  final VoidCallback? onTap;

  const _AreaLiveRow({
    required this.area,
    required this.reading,
    required this.powerOn,
    required this.boosted,
    this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    final brightness = powerOn
        ? (reading.brightness + area.brightnessOffset).clamp(0, 100)
        : 0;
    return GestureDetector(
      onTap: onTap,
      behavior: HitTestBehavior.opaque,
      child: Container(
        margin: const EdgeInsets.only(top: 8),
        padding: const EdgeInsets.all(12),
        decoration: BoxDecoration(
          color: _panel2Color.withValues(alpha: 0.72),
          borderRadius: BorderRadius.circular(8),
          border: Border.all(
            color: CelestialColors.orbitRing.withValues(alpha: 0.7),
          ),
        ),
        child: Row(
          children: [
            Container(
              width: 34,
              height: 34,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: powerOn
                    ? _cctToColor(reading.kelvin).withValues(alpha: 0.95)
                    : CelestialColors.orbitRing,
                boxShadow: powerOn
                    ? [
                        BoxShadow(
                          color: _cctToColor(reading.kelvin)
                              .withValues(alpha: boosted ? 0.6 : 0.28),
                          blurRadius: boosted ? 20 : 12,
                        ),
                      ]
                    : const [],
              ),
              child: Icon(
                Icons.lightbulb_rounded,
                color: _readableOn(_cctToColor(reading.kelvin)),
                size: 18,
              ),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    area.name,
                    style: const TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 14,
                      fontWeight: FontWeight.w700,
                    ),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    '${area.deviceCount} lights',
                    style: const TextStyle(
                      color: CelestialColors.textSecondary,
                      fontSize: 11,
                    ),
                  ),
                ],
              ),
            ),
            Text(
              powerOn ? '$brightness% / ${reading.kelvin} K' : 'off',
              style: const TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 13,
                fontWeight: FontWeight.w700,
                fontFeatures: [FontFeature.tabularFigures()],
              ),
            ),
            if (onTap != null) ...[
              const SizedBox(width: 6),
              const Icon(
                Icons.chevron_right_rounded,
                color: CelestialColors.textSecondary,
                size: 20,
              ),
            ],
          ],
        ),
      ),
    );
  }
}

class _ExpertHeader extends StatelessWidget {
  final String title;
  final Widget leading;
  final Widget trailing;
  final double leadingWidth;
  final double trailingWidth;

  const _ExpertHeader({
    required this.title,
    required this.leading,
    required this.trailing,
    this.leadingWidth = 48,
    this.trailingWidth = 96,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
      child: Row(
        children: [
          SizedBox(
              width: leadingWidth,
              child: Align(alignment: Alignment.centerLeft, child: leading)),
          Expanded(
            child: Text(
              title,
              textAlign: TextAlign.center,
              style: const TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 18,
                fontWeight: FontWeight.w700,
              ),
            ),
          ),
          SizedBox(
              width: trailingWidth,
              child: Align(alignment: Alignment.centerRight, child: trailing)),
        ],
      ),
    );
  }
}

class _BackButton extends StatelessWidget {
  final VoidCallback onPressed;
  final Color ink;
  final Color background;

  const _BackButton({
    required this.onPressed,
    this.ink = CelestialColors.accentBlue,
    this.background = const Color(0x3358A6FF),
  });

  @override
  Widget build(BuildContext context) {
    return IconButton(
      onPressed: onPressed,
      icon: Icon(Icons.chevron_left_rounded, color: ink),
      style: IconButton.styleFrom(
        backgroundColor: background,
        fixedSize: const Size(40, 40),
      ),
      tooltip: 'Back',
    );
  }
}

class _ExpertModeBadge extends StatelessWidget {
  const _ExpertModeBadge();

  @override
  Widget build(BuildContext context) {
    return Tooltip(
      message: 'Expert mode',
      child: Container(
        width: 40,
        height: 40,
        alignment: Alignment.center,
        decoration: BoxDecoration(
          color: _expertAccent.withValues(alpha: 0.14),
          borderRadius: BorderRadius.circular(10),
          border: Border.all(color: _expertAccent.withValues(alpha: 0.42)),
        ),
        child: const Icon(
          Icons.tune_rounded,
          color: _expertAccent,
          size: 20,
        ),
      ),
    );
  }
}

class _ExpertPanel extends StatelessWidget {
  final Widget child;
  final EdgeInsetsGeometry padding;

  const _ExpertPanel({
    required this.child,
    this.padding = const EdgeInsets.all(14),
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: padding,
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(10),
        border: Border.all(color: CelestialColors.orbitRing),
      ),
      child: child,
    );
  }
}

class _ZoneLinkButton extends StatelessWidget {
  final String label;
  final VoidCallback onTap;

  const _ZoneLinkButton({
    required this.label,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    return OutlinedButton(
      onPressed: onTap,
      style: OutlinedButton.styleFrom(
        foregroundColor: CelestialColors.textPrimary,
        side: const BorderSide(color: CelestialColors.orbitRing),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(7)),
        minimumSize: const Size(70, 34),
        padding: const EdgeInsets.symmetric(horizontal: 10),
      ),
      child: Text(label),
    );
  }
}

class _DefaultBadge extends StatelessWidget {
  const _DefaultBadge();

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 3),
      decoration: BoxDecoration(
        color: _expertAccent.withValues(alpha: 0.12),
        borderRadius: BorderRadius.circular(999),
        border: Border.all(color: _expertAccent.withValues(alpha: 0.6)),
      ),
      child: const Text(
        'DEFAULT',
        style: TextStyle(
          color: _expertAccent,
          fontSize: 9,
          fontWeight: FontWeight.w800,
          letterSpacing: 0.5,
        ),
      ),
    );
  }
}

class _SmallSquareButton extends StatelessWidget {
  final IconData icon;
  final VoidCallback? onPressed;

  const _SmallSquareButton({
    required this.icon,
    required this.onPressed,
  });

  @override
  Widget build(BuildContext context) {
    return IconButton(
      onPressed: onPressed,
      icon: Icon(icon),
      iconSize: 18,
      style: IconButton.styleFrom(
        foregroundColor: CelestialColors.textSecondary,
        disabledForegroundColor:
            CelestialColors.textSecondary.withValues(alpha: 0.25),
        backgroundColor: _panel2Color,
        fixedSize: const Size(28, 24),
        minimumSize: const Size(28, 24),
        padding: EdgeInsets.zero,
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(5)),
        side: const BorderSide(color: CelestialColors.orbitRing),
      ),
    );
  }
}

class _CompactTextButton extends StatelessWidget {
  final String label;
  final Color color;
  final VoidCallback? onPressed;

  const _CompactTextButton({
    required this.label,
    this.color = _expertAccent,
    required this.onPressed,
  });

  @override
  Widget build(BuildContext context) {
    return TextButton(
      onPressed: onPressed,
      style: TextButton.styleFrom(
        foregroundColor: color,
        disabledForegroundColor: color.withValues(alpha: 0.28),
        padding: const EdgeInsets.symmetric(horizontal: 10),
        minimumSize: const Size(0, 34),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(7)),
        side: BorderSide(color: color.withValues(alpha: 0.5)),
      ),
      child: Text(label),
    );
  }
}

class _LiveAction extends StatelessWidget {
  final String label;
  final String value;
  final bool active;
  final Color ink;
  final VoidCallback onTap;

  const _LiveAction({
    required this.label,
    required this.value,
    required this.active,
    required this.ink,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: onTap,
      behavior: HitTestBehavior.opaque,
      child: Padding(
        padding: const EdgeInsets.only(right: 16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              label,
              style: TextStyle(
                color: ink,
                fontSize: 12,
                fontWeight: FontWeight.w800,
              ),
            ),
            Text(
              value,
              style: TextStyle(
                color: ink.withValues(alpha: active ? 0.75 : 0.45),
                fontSize: 10,
                fontWeight: FontWeight.w600,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _LivePreviewButton extends StatelessWidget {
  final bool active;
  final bool busy;
  final bool enabled;
  final Future<void> Function() onPressed;

  const _LivePreviewButton({
    required this.active,
    required this.busy,
    required this.enabled,
    required this.onPressed,
  });

  @override
  Widget build(BuildContext context) {
    final label = active ? 'Stop preview' : 'Feel it';
    final icon = active ? Icons.stop_circle_outlined : Icons.sensors_rounded;
    return FilledButton.icon(
      onPressed: enabled && !busy ? onPressed : null,
      icon: busy
          ? const SizedBox(
              width: 18,
              height: 18,
              child: CircularProgressIndicator(strokeWidth: 2),
            )
          : Icon(icon),
      label: Text(label),
      style: FilledButton.styleFrom(
        backgroundColor: active ? _dangerAccent : _expertAccent,
        foregroundColor: active ? Colors.white : const Color(0xFF1A120C),
        disabledBackgroundColor:
            CelestialColors.orbitRing.withValues(alpha: 0.45),
        disabledForegroundColor:
            CelestialColors.textSecondary.withValues(alpha: 0.55),
        padding: const EdgeInsets.symmetric(vertical: 14),
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
      ),
    );
  }
}

class _ExpertZone {
  final String id;
  final String name;
  final bool isDefault;
  final List<_ExpertArea> areas;
  final _RhythmDefinition definition;
  final _ExpertZoneState? currentState;

  const _ExpertZone({
    required this.name,
    required this.isDefault,
    required this.areas,
    required this.definition,
    this.currentState,
    String? id,
  }) : id = id ?? name;

  _ExpertZone copyWith({
    String? id,
    String? name,
    bool? isDefault,
    List<_ExpertArea>? areas,
    _RhythmDefinition? definition,
    _ExpertZoneState? currentState,
  }) {
    return _ExpertZone(
      id: id ?? this.id,
      name: name ?? this.name,
      isDefault: isDefault ?? this.isDefault,
      areas: areas ?? this.areas,
      definition: definition ?? this.definition,
      currentState: currentState ?? this.currentState,
    );
  }
}

class _ExpertZoneState {
  final int brightness;
  final int actualBrightness;
  final int kelvin;
  final int minBrightness;
  final int maxBrightness;
  final double? brightnessSensitivity;
  final bool frozen;
  final bool hasRuntimeAdjustment;

  const _ExpertZoneState({
    required this.brightness,
    required this.actualBrightness,
    required this.kelvin,
    required this.minBrightness,
    required this.maxBrightness,
    required this.brightnessSensitivity,
    required this.frozen,
    required this.hasRuntimeAdjustment,
  });

  String get targetLabel => '$actualBrightness% / $kelvin K';

  String get stateLabel {
    if (frozen) return 'held';
    if (hasRuntimeAdjustment) return 'adjusted';
    return 'auto';
  }
}

class _ExpertServerSnapshot {
  final String? channel;
  final String? timezone;
  final double? serverHour;
  final double? latitude;
  final double? longitude;
  final int? brightness;
  final int? kelvin;

  const _ExpertServerSnapshot({
    required this.channel,
    required this.timezone,
    required this.serverHour,
    required this.latitude,
    required this.longitude,
    required this.brightness,
    required this.kelvin,
  });

  factory _ExpertServerSnapshot.fromModern({
    Map<dynamic, dynamic>? state,
    Map<dynamic, dynamic>? now,
  }) {
    final location = _asMapOrNull(state?['location']) ?? const {};
    final lighting = _asMapOrNull(now?['lighting']);
    return _ExpertServerSnapshot(
      channel:
          _stringValue(state?['platform']) ?? _stringValue(state?['context']),
      timezone: _stringValue(location['timezone']) ??
          _stringValue(location['timezone_name']) ??
          _stringValue(now?['timezone']),
      serverHour: _nullableDouble(now?['current_hour']) ??
          _nullableDouble(now?['hour']) ??
          _nullableDouble(lighting?['hour']),
      latitude: _nullableDouble(location['latitude']) ??
          _nullableDouble(location['lat']),
      longitude: _nullableDouble(location['longitude']) ??
          _nullableDouble(location['lon']) ??
          _nullableDouble(location['lng']),
      brightness: _nullableDouble(lighting?['brightness'] ?? now?['brightness'])
          ?.round()
          .clamp(0, 100)
          .toInt(),
      kelvin: _nullableDouble(lighting?['kelvin'] ?? now?['kelvin'])
          ?.round()
          .clamp(1500, 7000)
          .toInt(),
    );
  }

  bool get hasMetrics {
    return serverHour != null ||
        (brightness != null && kelvin != null) ||
        locationLabel != null;
  }

  String? get channelLabel {
    final value = channel?.trim().toUpperCase();
    if (value == null || value == 'MAIN') return null;
    return value;
  }

  String get timezoneLabel {
    final value = timezone;
    if (value == null || value.isEmpty) return 'server';
    final parts = value.split('/');
    return parts.isEmpty ? value : parts.last.replaceAll('_', ' ');
  }

  String? get locationLabel {
    final lat = latitude;
    final lon = longitude;
    if (lat == null || lon == null) return null;
    return '${_compactCoordinate(lat)}, ${_compactCoordinate(lon)}';
  }
}

class _ExpertArea {
  final String id;
  final String name;
  final int deviceCount;
  final int brightnessOffset;
  final List<_ExpertSectionOption> sections;
  final bool? lightsOn;
  final int? brightness;
  final int? kelvin;
  final bool stale;

  const _ExpertArea({
    required this.id,
    required this.name,
    required this.deviceCount,
    this.brightnessOffset = 0,
    this.sections = const [],
    this.lightsOn,
    this.brightness,
    this.kelvin,
    this.stale = false,
  });
}

class _ExpertZoneSchedule {
  final Map<dynamic, dynamic>? overrideData;
  final Map<dynamic, dynamic>? nextTimes;
  final String? error;

  const _ExpertZoneSchedule({
    required this.overrideData,
    required this.nextTimes,
  }) : error = null;

  const _ExpertZoneSchedule.unavailable(this.error)
      : overrideData = null,
        nextTimes = null;

  bool get hasOverride => overrideData != null && overrideData!.isNotEmpty;

  String? get mode => _stringValue(overrideData?['mode']);
  double? get customWake => _nullableDouble(overrideData?['custom_wake']);
  double? get customBed => _nullableDouble(overrideData?['custom_bed']);

  String get overrideLabel {
    if (!hasOverride) return 'none';
    final label = switch (mode) {
      'main' => 'main',
      'alt' => 'alt',
      'custom' => customWake != null && customBed != null
          ? '${_formatHour(customWake!)} / ${_formatHour(customBed!)}'
          : 'custom',
      'off' => 'off',
      _ => mode ?? 'set',
    };
    final until = _stringValue(overrideData?['until_date']);
    final event = _stringValue(overrideData?['until_event']);
    if (until == null || until.isEmpty) return label;
    return event == null ? '$label to $until' : '$label to $until $event';
  }

  String? get nextWakeLabel => _eventLabel('wake');
  String? get nextBedLabel => _eventLabel('bed');

  String? _eventLabel(String prefix) {
    final time = _nullableDouble(nextTimes?['${prefix}_time']);
    if (time == null) return null;
    final day = _nullableInt(nextTimes?['${prefix}_day']);
    final timeLabel = _formatHour(time);
    return day == null ? timeLabel : '${_shortWeekday(day)} $timeLabel';
  }
}

class _ExpertRhythmPresetLoad {
  final List<String> names;
  final Map<String, _ExpertRhythmPreset> presets;

  const _ExpertRhythmPresetLoad({
    required this.names,
    required this.presets,
  });

  factory _ExpertRhythmPresetLoad.fromProfiles(Map<dynamic, dynamic> raw) {
    final rawProfiles = raw['profiles'];
    final presets = <String, _ExpertRhythmPreset>{};
    final names = <String>[];
    if (rawProfiles is Iterable) {
      for (final item in rawProfiles) {
        final body = _asMapOrNull(item);
        if (body == null) continue;
        final preset = _ExpertRhythmPreset.fromModernProfile(body);
        presets[preset.name] = preset;
        names.add(preset.name);
      }
    }
    return _ExpertRhythmPresetLoad(
      names: List.unmodifiable(names),
      presets: Map.unmodifiable(presets),
    );
  }
}

class _ExpertRhythmPreset {
  final String name;
  final double wakeHour;
  final double bedHour;
  final double ascendStart;
  final double descendStart;

  const _ExpertRhythmPreset({
    required this.name,
    required this.wakeHour,
    required this.bedHour,
    required this.ascendStart,
    required this.descendStart,
  });

  factory _ExpertRhythmPreset.fromModernProfile(Map<dynamic, dynamic> raw) {
    final id = _stringValue(raw['id']) ??
        _stringValue(raw['name'])?.toLowerCase().replaceAll(' ', '_') ??
        'rhythm';
    final curve = _asMapOrNull(raw['curve']) ?? const {};
    final schedule = _asMapOrNull(curve['schedule']) ?? const {};
    final wake = _doubleValue(
      _asMapOrNull(schedule['wake'])?['hour'],
      fallback: _patternWake(id),
    );
    final bed = _doubleValue(
      _asMapOrNull(schedule['bed'])?['hour'],
      fallback: _patternBed(id),
    );
    return _ExpertRhythmPreset(
      name: id,
      wakeHour: _normalizeHour(wake),
      bedHour: _normalizeHour(bed),
      ascendStart: _normalizeHour(
        _doubleValue(curve['ascend_start'], fallback: wake - 3.5),
      ),
      descendStart: _normalizeHour(
        _doubleValue(curve['descend_start'], fallback: bed - 4),
      ),
    );
  }
}

class _ExpertSunTimes {
  final String date;
  final double sunrise;
  final double sunset;
  final double noon;
  final double midnight;

  const _ExpertSunTimes({
    required this.date,
    required this.sunrise,
    required this.sunset,
    required this.noon,
    required this.midnight,
  });

  factory _ExpertSunTimes.fromServer(Map<dynamic, dynamic> raw) {
    return _ExpertSunTimes(
      date: _stringValue(raw['date']) ?? '',
      sunrise: _doubleValue(raw['sunrise_hour'] ?? raw['sunrise'], fallback: 6),
      sunset: _doubleValue(raw['sunset_hour'] ?? raw['sunset'], fallback: 18),
      noon: _doubleValue(raw['noon_hour'] ?? raw['solar_noon'], fallback: 12),
      midnight: _doubleValue(
        raw['midnight_hour'] ?? raw['solar_midnight'],
        fallback: 0,
      ),
    );
  }
}

class _ExpertCurveData {
  final List<_ExpertCurvePoint> points;

  const _ExpertCurveData(this.points);

  factory _ExpertCurveData.fromServer(Map<dynamic, dynamic> raw) {
    final hours = _numberList(_pick(raw, const ['hours', 'hour']));
    final bris = _numberList(
      _pick(raw, const ['bris', 'bri', 'brightness', 'brightnesses']),
    );
    final ccts = _numberList(
      _pick(raw, const ['ccts', 'cct', 'kelvin', 'kelvins', 'color_temps']),
    );
    final generatedHours = hours.isNotEmpty
        ? hours
        : List<double>.generate(
            math.min(bris.length, ccts.length),
            (index) => bris.length <= 1 ? 0 : index * 24 / (bris.length - 1),
          );
    final length = math.min(
      generatedHours.length,
      math.min(bris.length, ccts.length),
    );
    if (length <= 0) return const _ExpertCurveData([]);

    final points = <_ExpertCurvePoint>[];
    for (int i = 0; i < length; i++) {
      final hour = generatedHours[i].clamp(0, 24).toDouble();
      final brightness = bris[i].round().clamp(0, 100).toInt();
      final kelvin = ccts[i].round().clamp(1500, 7000).toInt();
      points.add(
        _ExpertCurvePoint(
          hour: hour,
          brightness: brightness,
          kelvin: kelvin,
        ),
      );
    }
    points.sort((a, b) => a.hour.compareTo(b.hour));
    return _ExpertCurveData(List.unmodifiable(points));
  }

  _CurveReading? readingAt(double hour) {
    if (points.isEmpty) return null;
    if (points.length == 1) return points.first.reading;

    final normalized = _normalizeHour(hour);
    final target = hour >= 24 && normalized == 0 ? 24.0 : normalized;
    for (int i = 0; i < points.length - 1; i++) {
      final start = points[i];
      final end = points[i + 1];
      if (target >= start.hour && target <= end.hour) {
        return _interpolate(start, end, target);
      }
    }

    final first = points.first;
    final last = points.last;
    final wrappedTarget = target < first.hour ? target + 24 : target;
    final wrappedEnd = first.hour + 24;
    if (wrappedTarget >= last.hour && wrappedTarget <= wrappedEnd) {
      return _interpolate(last, first, wrappedTarget, endHour: wrappedEnd);
    }
    return _nearest(target).reading;
  }

  _ExpertCurvePoint _nearest(double hour) {
    var nearest = points.first;
    var nearestDistance = double.infinity;
    for (final point in points) {
      final distance = (point.hour - hour).abs();
      if (distance < nearestDistance) {
        nearest = point;
        nearestDistance = distance;
      }
    }
    return nearest;
  }

  static _CurveReading _interpolate(
    _ExpertCurvePoint start,
    _ExpertCurvePoint end,
    double target, {
    double? endHour,
  }) {
    final resolvedEndHour = endHour ?? end.hour;
    final span = resolvedEndHour - start.hour;
    if (span.abs() < 0.001) return start.reading;
    final progress = ((target - start.hour) / span).clamp(0, 1).toDouble();
    final brightness =
        start.brightness + (end.brightness - start.brightness) * progress;
    final kelvin = start.kelvin + (end.kelvin - start.kelvin) * progress;
    return _CurveReading(
      brightness: brightness.round().clamp(0, 100),
      kelvin: kelvin.round().clamp(1500, 7000),
    );
  }

  static Object? _pick(Map<dynamic, dynamic> raw, List<String> keys) {
    final sources = [
      raw,
      if (_asMapOrNull(raw['curve']) != null) _asMapOrNull(raw['curve'])!,
      if (_asMapOrNull(raw['data']) != null) _asMapOrNull(raw['data'])!,
    ];
    for (final source in sources) {
      for (final key in keys) {
        if (source.containsKey(key)) return source[key];
      }
    }
    return null;
  }

  static List<double> _numberList(Object? value) {
    if (value is Iterable) {
      return [
        for (final item in value)
          if (_nullableDouble(item) != null) _nullableDouble(item)!,
      ];
    }
    if (value is String) {
      return [
        for (final item in value.split(RegExp(r'[\s,]+')))
          if (_nullableDouble(item) != null) _nullableDouble(item)!,
      ];
    }
    return const [];
  }
}

class _ExpertCurvePoint {
  final double hour;
  final int brightness;
  final int kelvin;

  const _ExpertCurvePoint({
    required this.hour,
    required this.brightness,
    required this.kelvin,
  });

  _CurveReading get reading => _CurveReading(
        brightness: brightness,
        kelvin: kelvin,
      );
}

class _ExpertStepSequences {
  final List<_ExpertStepPoint> stepUp;
  final List<_ExpertStepPoint> stepDown;

  const _ExpertStepSequences({
    required this.stepUp,
    required this.stepDown,
  });

  factory _ExpertStepSequences.fromServer(Map<dynamic, dynamic> raw) {
    final source = _asMapOrNull(raw['steps']) ?? raw;
    return _ExpertStepSequences(
      stepUp: _stepPointList(_stepListSource(source, 'step_up')),
      stepDown: _stepPointList(_stepListSource(source, 'step_down')),
    );
  }

  static Object? _stepListSource(Map<dynamic, dynamic> raw, String key) {
    final value = raw[key];
    return _asMapOrNull(value)?['steps'] ?? value;
  }

  static List<_ExpertStepPoint> _stepPointList(Object? raw) {
    if (raw is! Iterable) return const [];
    return List.unmodifiable([
      for (final item in raw)
        if (_asMapOrNull(item) != null)
          _ExpertStepPoint.fromServer(_asMapOrNull(item)!),
    ]);
  }
}

class _ExpertStepPoint {
  final double hour;
  final int brightness;
  final int kelvin;

  const _ExpertStepPoint({
    required this.hour,
    required this.brightness,
    required this.kelvin,
  });

  factory _ExpertStepPoint.fromServer(Map<dynamic, dynamic> raw) {
    return _ExpertStepPoint(
      hour: _normalizeHour(_doubleValue(raw['hour'], fallback: 0)),
      brightness:
          _intValue(raw['brightness'], fallback: 0).clamp(0, 100).toInt(),
      kelvin:
          _intValue(raw['kelvin'], fallback: 4000).clamp(1500, 7000).toInt(),
    );
  }
}

class _ExpertConfig {
  final Map<String, Object?> values;

  const _ExpertConfig(this.values);
  const _ExpertConfig.empty() : values = const {};

  _ExpertConfig withField(String key, Object? value) {
    return _ExpertConfig(Map.unmodifiable({...values, key: value}));
  }

  int intValue(String key, {required int fallback}) {
    return _intValue(values[key], fallback: fallback);
  }

  double doubleValue(String key, {required double fallback}) {
    return _doubleValue(values[key], fallback: fallback);
  }

  bool boolValue(String key, {required bool fallback}) {
    return _boolValue(values[key], fallback: fallback);
  }

  String stringValue(String key, {required String fallback}) {
    return _stringValue(values[key]) ?? fallback;
  }
}

class _TuneStep {
  final String label;
  final double factor;
  final double exposure;

  const _TuneStep.lumen(this.label, this.factor) : exposure = 0;
  const _TuneStep.solar(this.label, this.exposure) : factor = 1;
}

const _lumenSteps = [
  _TuneStep.lumen('Min', 0.60),
  _TuneStep.lumen('Very Dim', 0.75),
  _TuneStep.lumen('Dim', 0.85),
  _TuneStep.lumen('Subtle', 0.95),
  _TuneStep.lumen('Normal', 1.00),
  _TuneStep.lumen('Lifted', 1.05),
  _TuneStep.lumen('Bright', 1.15),
  _TuneStep.lumen('Strong', 1.25),
  _TuneStep.lumen('Max', 1.40),
];

const _solarSteps = [
  _TuneStep.solar('None', 0.00),
  _TuneStep.solar('Low', 0.25),
  _TuneStep.solar('Moderate', 0.40),
  _TuneStep.solar('Strong', 0.60),
  _TuneStep.solar('High', 0.75),
  _TuneStep.solar('Near full', 0.90),
  _TuneStep.solar('Full', 1.00),
  _TuneStep.solar('Amplified', 1.25),
  _TuneStep.solar('Intense', 1.60),
  _TuneStep.solar('Sun room', 2.00),
];

class _ExpertAreaStatus {
  final bool isOn;
  final bool isCircadian;
  final bool frozen;
  final bool boosted;
  final int brightness;
  final int actualBrightness;
  final int kelvin;
  final int minBrightness;
  final int maxBrightness;
  final int minKelvin;
  final int maxKelvin;
  final double areaFactor;
  final String? phase;
  final double? brightnessOverride;
  final double? colorOverride;
  final double? brightnessMid;
  final double? phaseMidpoint;
  final double? adjustedWakeHour;
  final double? adjustedBedHour;
  final double? effectiveWakeHour;
  final double? effectiveBedHour;
  final double naturalLightExposure;
  final double? outdoorNormalized;
  final double? sunBrightFactor;
  final double? brightnessSensitivity;
  final String? outdoorSource;
  final String? nextAutoOn;
  final String? nextAutoOff;

  const _ExpertAreaStatus({
    required this.isOn,
    required this.isCircadian,
    required this.frozen,
    required this.boosted,
    required this.brightness,
    required this.actualBrightness,
    required this.kelvin,
    required this.minBrightness,
    required this.maxBrightness,
    required this.minKelvin,
    required this.maxKelvin,
    required this.areaFactor,
    required this.phase,
    required this.brightnessOverride,
    required this.colorOverride,
    required this.brightnessMid,
    required this.phaseMidpoint,
    required this.adjustedWakeHour,
    required this.adjustedBedHour,
    required this.effectiveWakeHour,
    required this.effectiveBedHour,
    required this.naturalLightExposure,
    required this.outdoorNormalized,
    required this.sunBrightFactor,
    required this.brightnessSensitivity,
    required this.outdoorSource,
    required this.nextAutoOn,
    required this.nextAutoOff,
  });

  double? get phaseTargetHour {
    if (phase == 'bed') {
      return adjustedBedHour ?? phaseMidpoint ?? effectiveBedHour;
    }
    return adjustedWakeHour ?? phaseMidpoint ?? effectiveWakeHour;
  }

  bool get hasBrightnessOverride => (brightnessOverride ?? 0).abs() > 0.1;
  bool get hasColorOverride => (colorOverride ?? 0).abs() > 0.1;

  bool get hasPhaseShift {
    final target = phaseTargetHour;
    final effective = phase == 'bed' ? effectiveBedHour : effectiveWakeHour;
    if (target == null || effective == null) return brightnessMid != null;
    return _circularDistance(target, effective) > 0.03;
  }

  bool get hasDivergence =>
      boosted || hasBrightnessOverride || hasColorOverride || hasPhaseShift;

  _ExpertAreaStatus copyWith({
    Object? brightnessMid = _unsetValue,
    Object? phaseMidpoint = _unsetValue,
  }) {
    return _ExpertAreaStatus(
      isOn: isOn,
      isCircadian: isCircadian,
      frozen: frozen,
      boosted: boosted,
      brightness: brightness,
      actualBrightness: actualBrightness,
      kelvin: kelvin,
      minBrightness: minBrightness,
      maxBrightness: maxBrightness,
      minKelvin: minKelvin,
      maxKelvin: maxKelvin,
      areaFactor: areaFactor,
      phase: phase,
      brightnessOverride: brightnessOverride,
      colorOverride: colorOverride,
      brightnessMid: identical(brightnessMid, _unsetValue)
          ? this.brightnessMid
          : brightnessMid as double?,
      phaseMidpoint: identical(phaseMidpoint, _unsetValue)
          ? this.phaseMidpoint
          : phaseMidpoint as double?,
      adjustedWakeHour: adjustedWakeHour,
      adjustedBedHour: adjustedBedHour,
      effectiveWakeHour: effectiveWakeHour,
      effectiveBedHour: effectiveBedHour,
      naturalLightExposure: naturalLightExposure,
      outdoorNormalized: outdoorNormalized,
      sunBrightFactor: sunBrightFactor,
      brightnessSensitivity: brightnessSensitivity,
      outdoorSource: outdoorSource,
      nextAutoOn: nextAutoOn,
      nextAutoOff: nextAutoOff,
    );
  }
}

_ExpertAreaStatus _areaStatusFromModernNode(Map<dynamic, dynamic> node) {
  final brightness = _intValue(node['brightness'], fallback: 50);
  final kelvin = _intValue(node['kelvin'], fallback: 4000);
  return _ExpertAreaStatus(
    isOn: _boolValue(node['lights_on'], fallback: true),
    isCircadian: _boolValue(node['rhythm_enabled'], fallback: true),
    frozen: false,
    boosted: false,
    brightness: brightness,
    actualBrightness: brightness,
    kelvin: kelvin,
    minBrightness: 1,
    maxBrightness: 100,
    minKelvin: 1500,
    maxKelvin: 7000,
    areaFactor: 1,
    phase: null,
    brightnessOverride: _nullableDouble(node['brightness_offset']),
    colorOverride: null,
    brightnessMid: null,
    phaseMidpoint: null,
    adjustedWakeHour: null,
    adjustedBedHour: null,
    effectiveWakeHour: null,
    effectiveBedHour: null,
    naturalLightExposure: 0,
    outdoorNormalized: null,
    sunBrightFactor: null,
    brightnessSensitivity: null,
    outdoorSource: null,
    nextAutoOn: null,
    nextAutoOff: null,
  );
}

_ExpertAreaStatus _areaStatusWithExpertScope(
  _ExpertAreaStatus base,
  Map<dynamic, dynamic> raw,
  String areaId,
) {
  final area = _scopeAreaById(raw, areaId);
  if (area == null) return base;
  return base.copyWith(
    brightnessMid: _scopeMidpoint(area['layer']),
    phaseMidpoint: _scopeMidpoint(area['effective']),
  );
}

_ExpertAreaStatus _aggregateModernStatuses(List<_ExpertAreaStatus> statuses) {
  if (statuses.isEmpty) return _areaStatusFromModernNode(const {});

  final brightness = _averageInt(
    statuses.map((status) => status.brightness),
    fallback: 50,
  );
  final actualBrightness = _averageInt(
    statuses.map((status) => status.actualBrightness),
    fallback: brightness,
  );
  return _ExpertAreaStatus(
    isOn: statuses.any((status) => status.isOn),
    isCircadian: statuses.any((status) => status.isCircadian),
    frozen: statuses.any((status) => status.frozen),
    boosted: statuses.any((status) => status.boosted),
    brightness: brightness,
    actualBrightness: actualBrightness,
    kelvin: _averageInt(
      statuses.map((status) => status.kelvin),
      fallback: 4000,
    ),
    minBrightness: 1,
    maxBrightness: 100,
    minKelvin: 1500,
    maxKelvin: 7000,
    areaFactor: 1,
    phase: null,
    brightnessOverride: null,
    colorOverride: null,
    brightnessMid: null,
    phaseMidpoint: null,
    adjustedWakeHour: null,
    adjustedBedHour: null,
    effectiveWakeHour: null,
    effectiveBedHour: null,
    naturalLightExposure: 0,
    outdoorNormalized: null,
    sunBrightFactor: null,
    brightnessSensitivity: null,
    outdoorSource: null,
    nextAutoOn: null,
    nextAutoOff: null,
  );
}

Map<dynamic, dynamic> _modernNowEntryFromNode(Map<dynamic, dynamic> node) {
  return {
    'brightness': node['brightness'],
    'kelvin': node['kelvin'],
    'is_on': node['lights_on'],
    'is_circadian': node['rhythm_enabled'],
    'ts': DateTime.now().millisecondsSinceEpoch / 1000,
  };
}

class _ExpertActivityFeed {
  final List<_ExpertActivityEntry> entries;
  final List<String> sources;
  final List<String> actions;
  final bool capped;

  const _ExpertActivityFeed({
    required this.entries,
    required this.sources,
    required this.actions,
    required this.capped,
  });

  const _ExpertActivityFeed.empty()
      : entries = const [],
        sources = const [],
        actions = const [],
        capped = false;

  factory _ExpertActivityFeed.fromServer(Map<dynamic, dynamic> raw) {
    final entries = <_ExpertActivityEntry>[];
    final rawEntries = raw['entries'] ?? raw['activities'];
    if (rawEntries is Iterable) {
      for (final entry in rawEntries) {
        if (entry is Map) entries.add(_ExpertActivityEntry.fromServer(entry));
      }
    }

    final facets = _asMapOrNull(raw['facets']) ?? const {};
    return _ExpertActivityFeed(
      entries: entries,
      sources: _stringList(facets['sources']),
      actions: _stringList(facets['actions']),
      capped: _boolValue(raw['capped'], fallback: false),
    );
  }
}

class _ExpertActivityEntry {
  final double ts;
  final String action;
  final String sourceKind;
  final String? sourceEntity;
  final String? areaId;
  final int? brightness;
  final int? kelvin;
  final double? fromValue;
  final double? toValue;
  final int? durationMinutes;
  final int? intensity;
  final bool isZoneAction;
  final int? count;
  final String? eventId;
  final Map<dynamic, dynamic>? payload;

  const _ExpertActivityEntry({
    required this.ts,
    required this.action,
    required this.sourceKind,
    required this.sourceEntity,
    required this.areaId,
    required this.brightness,
    required this.kelvin,
    required this.fromValue,
    required this.toValue,
    required this.durationMinutes,
    required this.intensity,
    required this.isZoneAction,
    required this.count,
    required this.eventId,
    required this.payload,
  });

  factory _ExpertActivityEntry.fromServer(Map<dynamic, dynamic> raw) {
    final source = _asMapOrNull(raw['source']);
    final change = _asMapOrNull(raw['change']);
    final nodeId = _stringValue(raw['area_id']) ?? _stringValue(raw['node_id']);
    final ts = _nullableDouble(raw['ts']) ??
        ((_nullableDouble(raw['epoch_ms']) ?? 0) / 1000);
    return _ExpertActivityEntry(
      ts: ts,
      action: _stringValue(raw['action']) ??
          _stringValue(raw['action_id']) ??
          'event',
      sourceKind: _stringValue(raw['source_kind']) ??
          _activitySourceKind(source?['kind']) ??
          'system',
      sourceEntity: _stringValue(raw['source_entity']) ??
          _stringValue(source?['control_id']) ??
          _stringValue(source?['raw']),
      areaId: nodeId,
      brightness: _nullableInt(raw['brightness']),
      kelvin: _nullableInt(raw['kelvin']),
      fromValue: _nullableDouble(raw['from_value']) ??
          _nullableDouble(change?['before']),
      toValue:
          _nullableDouble(raw['to_value']) ?? _nullableDouble(change?['after']),
      durationMinutes: _nullableInt(raw['duration_minutes']),
      intensity: _nullableInt(raw['intensity']),
      isZoneAction: _boolValue(
        raw['is_zone_action'],
        fallback: nodeId?.startsWith('zone') ?? false,
      ),
      count: _nullableInt(raw['count']),
      eventId: _stringValue(raw['event_id']) ?? _stringValue(raw['id']),
      payload: _asMapOrNull(raw['payload']),
    );
  }
}

class _ExpertActivityGroup {
  final String label;
  final List<_ExpertActivityEntry> entries;

  const _ExpertActivityGroup({
    required this.label,
    required this.entries,
  });
}

String? _activitySourceKind(Object? raw) {
  if (raw is String) return _stringValue(raw);
  if (raw is Map) {
    for (final value in raw.values) {
      final text = _stringValue(value);
      if (text != null) return text;
    }
  }
  return null;
}

class _ExpertMomentsLoad {
  final List<_ExpertMoment> moments;

  const _ExpertMomentsLoad({required this.moments});

  const _ExpertMomentsLoad.empty() : moments = const [];

  factory _ExpertMomentsLoad.fromModernScenes(
    Map<dynamic, dynamic> raw,
    Map<String, int> usageByMoment,
  ) {
    final moments = <_ExpertMoment>[];
    final rawScenes = raw['scenes'];
    if (rawScenes is Iterable) {
      for (final item in rawScenes) {
        final body = _asMapOrNull(item);
        if (body == null) continue;
        final id = _stringValue(body['id']);
        if (id == null || id.isEmpty) continue;
        moments.add(
          _ExpertMoment.fromModernScene(
            id,
            body,
            usageCount: usageByMoment[id] ?? 0,
          ),
        );
      }
    }
    return _ExpertMomentsLoad(moments: moments);
  }
}

class _ExpertMoment {
  final String id;
  final String name;
  final String icon;
  final String category;
  final String defaultAction;
  final int timerSeconds;
  final Map<String, _ExpertMomentException> exceptions;
  final int usageCount;

  const _ExpertMoment({
    required this.id,
    required this.name,
    required this.icon,
    required this.category,
    required this.defaultAction,
    required this.timerSeconds,
    required this.exceptions,
    required this.usageCount,
  });

  factory _ExpertMoment.fromModernScene(
    String id,
    Map<dynamic, dynamic> raw, {
    required int usageCount,
  }) {
    final light = _asMapOrNull(raw['light']);
    final entries = light?['entries'];
    final hasOffOutput = _modernSceneHasOffOutput(light);
    final extension = _asMapOrNull(
      _asMapOrNull(raw['extensions'])?[_expertMomentExtensionKey],
    );
    return _ExpertMoment(
      id: id,
      name: _stringValue(raw['name']) ?? id,
      icon: _stringValue(extension?['icon']) ?? 'mdi:palette',
      category: _stringValue(extension?['category']) ??
          _stringValue(_asMapOrNull(raw['source'])?['provider']) ??
          'scene',
      defaultAction: _stringValue(extension?['default_action']) ??
          _modernScenePresetAction(light) ??
          (hasOffOutput ? 'off' : 'lights_on'),
      timerSeconds: _intValue(extension?['timer'], fallback: 0),
      exceptions: _momentExceptionsFromServer(extension?['exceptions']),
      usageCount: usageCount + (entries is Iterable ? entries.length : 0),
    );
  }

  _ExpertMoment copyWith({
    String? name,
    String? icon,
    String? category,
    String? defaultAction,
    int? timerSeconds,
    Map<String, _ExpertMomentException>? exceptions,
    int? usageCount,
  }) {
    return _ExpertMoment(
      id: id,
      name: name ?? this.name,
      icon: icon ?? this.icon,
      category: category ?? this.category,
      defaultAction: defaultAction ?? this.defaultAction,
      timerSeconds: timerSeconds ?? this.timerSeconds,
      exceptions: exceptions ?? this.exceptions,
      usageCount: usageCount ?? this.usageCount,
    );
  }
}

class _ExpertMomentException {
  final String action;
  final int timerSeconds;

  const _ExpertMomentException({
    required this.action,
    required this.timerSeconds,
  });

  _ExpertMomentException copyWith({
    String? action,
    int? timerSeconds,
  }) {
    return _ExpertMomentException(
      action: action ?? this.action,
      timerSeconds: timerSeconds ?? this.timerSeconds,
    );
  }

  Map<String, Object?> toServer() {
    return {
      'action': action,
      'timer': timerSeconds,
    };
  }
}

class _ExpertMomentAssignment {
  final _ExpertControl control;
  final String event;

  const _ExpertMomentAssignment({
    required this.control,
    required this.event,
  });

  String get key => '${control.id}:$event';
}

class _ExpertSwitchesLoad {
  final List<_ExpertControl> controls;
  final Map<String, _ExpertSwitchType> types;
  final List<_ExpertMoment> moments;
  final int defaultPauseMinutes;
  final int refreshIntervalSeconds;
  final int controlsPulseWindowHours;
  final int controlsRecentWindowMinutes;

  const _ExpertSwitchesLoad({
    required this.controls,
    required this.types,
    required this.moments,
    required this.defaultPauseMinutes,
    required this.refreshIntervalSeconds,
    required this.controlsPulseWindowHours,
    required this.controlsRecentWindowMinutes,
  });

  const _ExpertSwitchesLoad.empty()
      : controls = const [],
        types = const {},
        moments = const [],
        defaultPauseMinutes = 240,
        refreshIntervalSeconds = 3,
        controlsPulseWindowHours = 6,
        controlsRecentWindowMinutes = 5;

  factory _ExpertSwitchesLoad.fromModern({
    required Map<dynamic, dynamic> bindingsRaw,
    required Map<dynamic, dynamic> actionsRaw,
    required Map<dynamic, dynamic> switchmapRaw,
    required Map<dynamic, dynamic> scenesRaw,
    required Map<dynamic, dynamic> configRaw,
    required List<RhythmTopologyNode> topologyNodes,
    required Map<dynamic, dynamic> scopeRaw,
  }) {
    return _ExpertSwitchesLoad(
      controls: _expertControlsFromModernBindings(
        bindingsRaw,
        topologyNodes,
        scopeRaw: scopeRaw,
      ),
      types: _expertSwitchTypesFromModernActions(
        actionsRaw,
        switchmapRaw: switchmapRaw,
      ),
      moments: _ExpertMomentsLoad.fromModernScenes(scenesRaw, const {}).moments,
      defaultPauseMinutes: _intValue(
        configRaw['default_pause_duration_minutes'],
        fallback: 240,
      ),
      refreshIntervalSeconds: 3,
      controlsPulseWindowHours: _intValue(
        configRaw['controls_pulse_window_hours'],
        fallback: 6,
      ).clamp(1, 168).toInt(),
      controlsRecentWindowMinutes: _intValue(
        configRaw['controls_recent_window_minutes'],
        fallback: 5,
      ).clamp(1, 1440).toInt(),
    );
  }

  _ExpertSwitchesLoad withUpdatedControl(_ExpertControl control) {
    return _ExpertSwitchesLoad(
      controls: [
        for (final candidate in controls)
          candidate.id == control.id ? control : candidate,
      ],
      types: types,
      moments: moments,
      defaultPauseMinutes: defaultPauseMinutes,
      refreshIntervalSeconds: refreshIntervalSeconds,
      controlsPulseWindowHours: controlsPulseWindowHours,
      controlsRecentWindowMinutes: controlsRecentWindowMinutes,
    );
  }

  _ExpertSwitchesLoad withRefresh(_ExpertControlsRefresh refresh) {
    var changed = false;
    final nextControls = <_ExpertControl>[];
    for (final control in controls) {
      var next = control;
      final keys = _controlRefreshKeys(control);
      final lastAction = _firstMapForKeys(refresh.lastActions, keys);
      if (lastAction != null &&
          !_mapStringEquals(lastAction, control.lastAction)) {
        next = next.copyWith(lastAction: lastAction);
      }

      final pause = _firstPauseForKeys(refresh.pauseStates, keys);
      if (pause != null) {
        final status = _controlStatusWithPause(control.status, pause.inactive);
        if (next.inactive != pause.inactive ||
            next.inactiveUntil != pause.inactiveUntil ||
            next.status != status) {
          next = next.copyWith(
            inactive: pause.inactive,
            inactiveUntil: pause.inactiveUntil,
            status: status,
          );
        }
      } else if (next.inactive) {
        next = next.copyWith(
          inactive: false,
          inactiveUntil: null,
          status: next.status == 'inactive' ? 'active' : next.status,
        );
      }

      if (!identical(next, control)) changed = true;
      nextControls.add(next);
    }

    if (!changed) return this;
    return _ExpertSwitchesLoad(
      controls: nextControls,
      types: types,
      moments: moments,
      defaultPauseMinutes: defaultPauseMinutes,
      refreshIntervalSeconds: refreshIntervalSeconds,
      controlsPulseWindowHours: controlsPulseWindowHours,
      controlsRecentWindowMinutes: controlsRecentWindowMinutes,
    );
  }
}

class _ExpertAreaControlsLoad {
  final List<_ExpertControl> controls;
  final int defaultPauseMinutes;
  final String? error;

  const _ExpertAreaControlsLoad({
    required this.controls,
    required this.defaultPauseMinutes,
  }) : error = null;

  const _ExpertAreaControlsLoad.unavailable(this.error)
      : controls = const [],
        defaultPauseMinutes = 240;
}

class _ExpertControlsRefresh {
  final Map<String, Map<dynamic, dynamic>> lastActions;
  final Map<String, _ExpertControlPauseState> pauseStates;

  const _ExpertControlsRefresh({
    required this.lastActions,
    required this.pauseStates,
  });

  factory _ExpertControlsRefresh.fromServer(Map<dynamic, dynamic> raw) {
    final lastActions = <String, Map<dynamic, dynamic>>{};
    final lastActionsRaw = _asMapOrNull(raw['last_actions']) ?? const {};
    for (final entry in lastActionsRaw.entries) {
      final key = _stringValue(entry.key);
      if (key == null) continue;
      final body = entry.value is Map
          ? Map<dynamic, dynamic>.from(entry.value as Map)
          : {'action': entry.value};
      if (body['timestamp'] == null && body['epoch_ms'] != null) {
        body['timestamp'] = _isoFromEpochMs(body['epoch_ms']);
      }
      lastActions[key] = body;
    }

    final pauseStates = <String, _ExpertControlPauseState>{};
    final pauseStatesRaw = _asMapOrNull(raw['pause_states']) ?? const {};
    for (final entry in pauseStatesRaw.entries) {
      final key = _stringValue(entry.key);
      final body = _asMapOrNull(entry.value);
      if (key == null || body == null) continue;
      final active = _boolValue(body['active'], fallback: true);
      final inactive = _boolValue(
        body['inactive'],
        fallback: !active,
      );
      pauseStates[key] = _ExpertControlPauseState(
        inactive: inactive,
        inactiveUntil: _stringValue(body['inactive_until']) ??
            _isoFromEpochMs(body['expires_at_epoch_ms']),
      );
    }

    return _ExpertControlsRefresh(
      lastActions: lastActions,
      pauseStates: pauseStates,
    );
  }
}

class _ExpertControlPauseState {
  final bool inactive;
  final String? inactiveUntil;

  const _ExpertControlPauseState({
    required this.inactive,
    required this.inactiveUntil,
  });
}

class _ExpertControl {
  final String id;
  final String name;
  final String category;
  final String status;
  final String type;
  final String typeName;
  final String? deviceId;
  final String? areaId;
  final String? areaName;
  final String? manufacturer;
  final String? model;
  final String? integration;
  final bool supported;
  final bool inactive;
  final String? inactiveUntil;
  final bool stale;
  final int? batteryLevel;
  final double? illuminance;
  final Map<dynamic, dynamic>? lastAction;
  final List<_ExpertSwitchScope> scopes;
  final List<_ExpertTriggerEntity> binarySensors;
  final Map<String, String?> magicButtons;
  final Map<dynamic, dynamic>? rawBinding;

  const _ExpertControl({
    required this.id,
    required this.name,
    required this.category,
    required this.status,
    required this.type,
    required this.typeName,
    required this.deviceId,
    required this.areaId,
    required this.areaName,
    required this.manufacturer,
    required this.model,
    required this.integration,
    required this.supported,
    required this.inactive,
    required this.inactiveUntil,
    required this.stale,
    required this.batteryLevel,
    required this.illuminance,
    required this.lastAction,
    required this.scopes,
    required this.binarySensors,
    required this.magicButtons,
    required this.rawBinding,
  });

  bool get isSwitch => category == 'switch';

  String get categoryLabel => _controlCategoryLabel(category);

  DateTime? get lastActionTime {
    final ts = _stringValue(lastAction?['timestamp']);
    return ts == null ? null : DateTime.tryParse(ts);
  }

  int get momentAssignmentCount => magicButtons.values
      .where((value) => value != null && value.startsWith('set_'))
      .length;

  String get pauseEndpointKey {
    const sensorCategories = {'motion_sensor', 'contact_sensor', 'camera'};
    if (sensorCategories.contains(category) && deviceId != null) {
      return deviceId!;
    }
    return id;
  }

  _ExpertControl copyWith({
    String? status,
    bool? inactive,
    Object? inactiveUntil = _unsetValue,
    Object? lastAction = _unsetValue,
    List<_ExpertSwitchScope>? scopes,
  }) {
    return _ExpertControl(
      id: id,
      name: name,
      category: category,
      status: status ?? this.status,
      type: type,
      typeName: typeName,
      deviceId: deviceId,
      areaId: areaId,
      areaName: areaName,
      manufacturer: manufacturer,
      model: model,
      integration: integration,
      supported: supported,
      inactive: inactive ?? this.inactive,
      inactiveUntil: identical(inactiveUntil, _unsetValue)
          ? this.inactiveUntil
          : inactiveUntil as String?,
      stale: stale,
      batteryLevel: batteryLevel,
      illuminance: illuminance,
      lastAction: identical(lastAction, _unsetValue)
          ? this.lastAction
          : lastAction as Map<dynamic, dynamic>?,
      scopes: scopes ?? this.scopes,
      binarySensors: binarySensors,
      magicButtons: magicButtons,
      rawBinding: rawBinding,
    );
  }

  Map<String, Object?> toConfigureServer() {
    return {
      'name': name,
      'category': category,
      'scopes': [for (final scope in scopes) scope.toSensorServer()],
      if (deviceId != null) 'device_id': deviceId,
      'inactive': inactive,
      'inactive_until': inactiveUntil,
    };
  }

  _ExpertSwitch toSwitch() {
    return _ExpertSwitch(
      id: id,
      name: name,
      type: type,
      typeName: typeName,
      currentScope: scopes.isEmpty ? 0 : 1,
      totalScopes: scopes.length,
      scopes: scopes,
      deviceId: deviceId,
      areaName: areaName,
      inactive: inactive,
      inactiveUntil: inactiveUntil,
      stale: stale,
      magicButtons: magicButtons,
      rawBinding: rawBinding,
    );
  }
}

class _ExpertTriggerEntity {
  final String entityId;
  final String name;

  const _ExpertTriggerEntity({
    required this.entityId,
    required this.name,
  });
}

class _ExpertControlSourceDevice {
  final String deviceId;
  final String name;
  final String? kind;
  final String? manufacturer;
  final String? model;
  final String? areaId;
  final String? areaName;
  final List<_ExpertControlSourceSensor> binarySensors;

  const _ExpertControlSourceDevice({
    required this.deviceId,
    required this.name,
    required this.kind,
    required this.manufacturer,
    required this.model,
    required this.areaId,
    required this.areaName,
    required this.binarySensors,
  });
}

class _ExpertControlSourceSensor {
  final String entityId;
  final String name;
  final String? deviceClass;

  const _ExpertControlSourceSensor({
    required this.entityId,
    required this.name,
    required this.deviceClass,
  });

  String get label => name.isEmpty ? _entityName(entityId) : name;
}

class _ExpertZhaSettings {
  final bool isZha;
  final String? ieee;
  final _ExpertZhaSensitivity? sensitivity;
  final int? timeoutSeconds;

  const _ExpertZhaSettings({
    required this.isZha,
    required this.ieee,
    required this.sensitivity,
    required this.timeoutSeconds,
  });

  factory _ExpertZhaSettings.fromServer(Map<dynamic, dynamic> raw) {
    final settings = _asMapOrNull(raw['settings']);
    final capabilities = raw['capabilities'];
    if (settings != null || capabilities is Iterable) {
      final sensitivityCapability = _hardwareCapability(raw, 'sensitivity');
      return _ExpertZhaSettings(
        isZha: _boolValue(raw['supported'], fallback: false),
        ieee: _stringValue(raw['native_id']),
        sensitivity: sensitivityCapability == null
            ? null
            : _ExpertZhaSensitivity.fromHardwareCapability(
                sensitivityCapability,
                settings,
              ),
        timeoutSeconds: _nullableInt(settings?['occupancy_timeout_secs']),
      );
    }

    final sensitivity = _asMapOrNull(raw['sensitivity']);
    return _ExpertZhaSettings(
      isZha: _boolValue(raw['is_zha'], fallback: false),
      ieee: _stringValue(raw['ieee']),
      sensitivity: sensitivity == null
          ? null
          : _ExpertZhaSensitivity.fromServer(sensitivity),
      timeoutSeconds: _nullableInt(raw['timeout']),
    );
  }

  bool get hasControls =>
      isZha &&
      (sensitivity?.options.isNotEmpty == true || timeoutSeconds != null);
}

class _ExpertZhaSensitivity {
  final String? entityId;
  final String? value;
  final List<String> options;
  final num? min;
  final num? max;
  final num? step;

  const _ExpertZhaSensitivity({
    required this.entityId,
    required this.value,
    required this.options,
    required this.min,
    required this.max,
    required this.step,
  });

  factory _ExpertZhaSensitivity.fromServer(Map<dynamic, dynamic> raw) {
    final explicitOptions = _stringList(raw['options']);
    final generatedOptions = explicitOptions.isNotEmpty
        ? explicitOptions
        : _zhaNumericOptions(
            min: _nullableDouble(raw['min']),
            max: _nullableDouble(raw['max']),
            step: _nullableDouble(raw['step']),
          );
    final value = _stringValue(raw['value']);
    final options = <String>[
      if (value != null && !generatedOptions.contains(value)) value,
      ...generatedOptions,
    ];
    return _ExpertZhaSensitivity(
      entityId: _stringValue(raw['entity_id']),
      value: value,
      options: options,
      min: _nullableDouble(raw['min']),
      max: _nullableDouble(raw['max']),
      step: _nullableDouble(raw['step']),
    );
  }

  factory _ExpertZhaSensitivity.fromHardwareCapability(
    Map<dynamic, dynamic> raw,
    Map<dynamic, dynamic>? settings,
  ) {
    final value = _stringValue(settings?['sensitivity']);
    final options = _stringList(raw['options']);
    return _ExpertZhaSensitivity(
      entityId: null,
      value: value,
      options: [
        if (value != null && !options.contains(value)) value,
        ...options,
      ],
      min: _nullableDouble(raw['min']),
      max: _nullableDouble(raw['max']),
      step: 1,
    );
  }
}

class _ExpertOutdoorStatus {
  final double outdoorNormalized;
  final String source;
  final String preferredSource;
  final _ExpertOutdoorOverride? override;
  final String? weatherCondition;
  final double? luxSmoothed;
  final double? luxLearnedCeiling;
  final double? luxLearnedFloor;
  final double sunElevation;
  final String? sensorEntity;
  final List<_ExpertWeatherGroup> weatherGroups;
  final double conditionMultiplier;
  final double angleFactor;

  const _ExpertOutdoorStatus({
    required this.outdoorNormalized,
    required this.source,
    required this.preferredSource,
    required this.override,
    required this.weatherCondition,
    required this.luxSmoothed,
    required this.luxLearnedCeiling,
    required this.luxLearnedFloor,
    required this.sunElevation,
    required this.sensorEntity,
    required this.weatherGroups,
    required this.conditionMultiplier,
    required this.angleFactor,
  });

  factory _ExpertOutdoorStatus.fromServer(Map<dynamic, dynamic> raw) {
    final diagnostics = _asMapOrNull(raw['diagnostics']) ?? const {};
    final groups = <_ExpertWeatherGroup>[];
    final rawGroups = raw['weather_groups'];
    if (rawGroups is Iterable) {
      for (final item in rawGroups) {
        final body = _asMapOrNull(item);
        if (body != null) groups.add(_ExpertWeatherGroup.fromServer(body));
      }
    }
    if (groups.isEmpty) groups.addAll(_modernWeatherGroups);

    final source = _stringValue(raw['source']) ?? 'none';
    final weatherCondition =
        _stringValue(raw['weather_condition'] ?? diagnostics['sky_condition']);
    final override = _ExpertOutdoorOverride.fromServer(raw['override']) ??
        (source == 'manual_override' && weatherCondition != null
            ? _ExpertOutdoorOverride(
                condition: weatherCondition,
                expiresInMinutes: null,
              )
            : null);
    return _ExpertOutdoorStatus(
      outdoorNormalized: _doubleValue(
        raw['outdoor_factor'] ?? raw['outdoor_normalized'],
        fallback: 0,
      ),
      source: source,
      preferredSource: _stringValue(raw['preferred_source']) ?? source,
      override: override,
      weatherCondition: weatherCondition,
      luxSmoothed: _nullableDouble(raw['lux_smoothed'] ?? diagnostics['lux']),
      luxLearnedCeiling: _nullableDouble(
        raw['lux_learned_ceiling'] ?? diagnostics['lux_learned_ceiling'],
      ),
      luxLearnedFloor: _nullableDouble(
        raw['lux_learned_floor'] ?? diagnostics['lux_learned_floor'],
      ),
      sunElevation: _doubleValue(
        raw['sun_elevation'] ?? diagnostics['sun_elevation_degrees'],
        fallback: 0,
      ),
      sensorEntity: _stringValue(raw['sensor_entity']),
      weatherGroups: groups,
      conditionMultiplier: _doubleValue(
        raw['condition_multiplier'] ?? diagnostics['sky_multiplier'],
        fallback: 1,
      ),
      angleFactor: _doubleValue(
        raw['angle_factor'] ?? diagnostics['sun_angle_factor'],
        fallback: 0,
      ),
    );
  }
}

class _ExpertOutdoorOverride {
  final String condition;
  final double? expiresInMinutes;

  const _ExpertOutdoorOverride({
    required this.condition,
    required this.expiresInMinutes,
  });

  static _ExpertOutdoorOverride? fromServer(Object? raw) {
    final body = _asMapOrNull(raw);
    if (body == null) return null;
    final condition = _stringValue(body['condition']);
    if (condition == null || condition.isEmpty) return null;
    return _ExpertOutdoorOverride(
      condition: condition,
      expiresInMinutes: _nullableDouble(body['expires_in_minutes']),
    );
  }
}

class _ExpertWeatherGroup {
  final String key;
  final String label;
  final double multiplier;

  const _ExpertWeatherGroup({
    required this.key,
    required this.label,
    required this.multiplier,
  });

  factory _ExpertWeatherGroup.fromServer(Map<dynamic, dynamic> raw) {
    return _ExpertWeatherGroup(
      key: _stringValue(raw['key']) ?? '',
      label: _stringValue(raw['label']) ?? '',
      multiplier: _doubleValue(raw['multiplier'], fallback: 1),
    );
  }
}

class _TriggerEntityGroup {
  final String id;
  final String label;
  final bool isAny;
  final List<String> ownEntityIds;
  final List<String> allEntityIds;

  const _TriggerEntityGroup({
    required this.id,
    required this.label,
    required this.isAny,
    required this.ownEntityIds,
    required this.allEntityIds,
  });
}

class _ExpertSwitch {
  final String id;
  final String name;
  final String type;
  final String typeName;
  final int currentScope;
  final int totalScopes;
  final List<_ExpertSwitchScope> scopes;
  final String? deviceId;
  final String? areaName;
  final bool inactive;
  final String? inactiveUntil;
  final bool stale;
  final Map<String, String?> magicButtons;
  final Map<dynamic, dynamic>? rawBinding;

  const _ExpertSwitch({
    required this.id,
    required this.name,
    required this.type,
    required this.typeName,
    required this.currentScope,
    required this.totalScopes,
    required this.scopes,
    required this.deviceId,
    required this.areaName,
    required this.inactive,
    required this.inactiveUntil,
    required this.stale,
    required this.magicButtons,
    required this.rawBinding,
  });

  int get momentAssignmentCount => magicButtons.values
      .where((value) => value != null && value.startsWith('set_'))
      .length;

  _ExpertSwitch copyWith({
    String? name,
    String? type,
    String? typeName,
    List<_ExpertSwitchScope>? scopes,
    Map<String, String?>? magicButtons,
  }) {
    return _ExpertSwitch(
      id: id,
      name: name ?? this.name,
      type: type ?? this.type,
      typeName: typeName ?? this.typeName,
      currentScope: currentScope,
      totalScopes: totalScopes,
      scopes: scopes ?? this.scopes,
      deviceId: deviceId,
      areaName: areaName,
      inactive: inactive,
      inactiveUntil: inactiveUntil,
      stale: stale,
      magicButtons: magicButtons ?? this.magicButtons,
      rawBinding: rawBinding,
    );
  }

  Map<String, Object?> toServer() {
    return {
      'name': name,
      'type': type,
      'scopes': [for (final scope in scopes) scope.toServer()],
      'magic_buttons': {
        for (final entry in magicButtons.entries)
          if (entry.value != null && entry.value!.isNotEmpty)
            entry.key: entry.value,
      },
      if (deviceId != null) 'device_id': deviceId,
    };
  }
}

class _ExpertSwitchScope {
  final List<String> areaIds;
  final List<String> sectionIds;
  final String? feedbackArea;
  final String mode;
  final int durationSeconds;
  final int cooldownSeconds;
  final bool boostEnabled;
  final int boostBrightness;
  final String alertIntensity;
  final int alertCount;
  final String activeWhen;
  final int activeOffset;
  final List<String> triggerEntities;

  const _ExpertSwitchScope({
    required this.areaIds,
    required this.sectionIds,
    required this.feedbackArea,
    this.mode = 'on_off',
    this.durationSeconds = 60,
    this.cooldownSeconds = 0,
    this.boostEnabled = false,
    this.boostBrightness = 50,
    this.alertIntensity = 'low',
    this.alertCount = 3,
    this.activeWhen = 'always',
    this.activeOffset = 0,
    this.triggerEntities = const [],
  });

  const _ExpertSwitchScope.empty()
      : areaIds = const [],
        sectionIds = const [],
        feedbackArea = null,
        mode = 'on_off',
        durationSeconds = 60,
        cooldownSeconds = 0,
        boostEnabled = false,
        boostBrightness = 50,
        alertIntensity = 'low',
        alertCount = 3,
        activeWhen = 'always',
        activeOffset = 0,
        triggerEntities = const [];

  _ExpertSwitchScope copyWith({
    List<String>? areaIds,
    List<String>? sectionIds,
    Object? feedbackArea = _unsetValue,
    String? mode,
    int? durationSeconds,
    int? cooldownSeconds,
    bool? boostEnabled,
    int? boostBrightness,
    String? alertIntensity,
    int? alertCount,
    String? activeWhen,
    int? activeOffset,
    List<String>? triggerEntities,
  }) {
    return _ExpertSwitchScope(
      areaIds: areaIds ?? this.areaIds,
      sectionIds: sectionIds ?? this.sectionIds,
      feedbackArea: identical(feedbackArea, _unsetValue)
          ? this.feedbackArea
          : feedbackArea as String?,
      mode: mode ?? this.mode,
      durationSeconds: durationSeconds ?? this.durationSeconds,
      cooldownSeconds: cooldownSeconds ?? this.cooldownSeconds,
      boostEnabled: boostEnabled ?? this.boostEnabled,
      boostBrightness: boostBrightness ?? this.boostBrightness,
      alertIntensity: alertIntensity ?? this.alertIntensity,
      alertCount: alertCount ?? this.alertCount,
      activeWhen: activeWhen ?? this.activeWhen,
      activeOffset: activeOffset ?? this.activeOffset,
      triggerEntities: triggerEntities ?? this.triggerEntities,
    );
  }

  Map<String, Object?> toServer() {
    return {
      'targets': [
        for (final areaId in areaIds) {'kind': 'area', 'id': areaId},
        for (final sectionId in sectionIds)
          {'kind': 'section', 'id': sectionId},
      ],
      'areas': areaIds,
      if (feedbackArea != null) 'feedback_area': feedbackArea,
    };
  }

  Map<String, Object?> toSensorServer() {
    return {
      'targets': [
        for (final areaId in areaIds) {'kind': 'area', 'id': areaId},
        for (final sectionId in sectionIds)
          {'kind': 'section', 'id': sectionId},
      ],
      'areas': areaIds,
      'mode': mode,
      'duration': durationSeconds,
      'cooldown': cooldownSeconds,
      'boost_enabled': boostEnabled,
      'boost_brightness': boostBrightness,
      'alert_intensity': alertIntensity,
      'alert_count': alertCount,
      'active_when': activeWhen,
      'active_offset': activeOffset,
      'trigger_entities': triggerEntities,
    };
  }
}

class _ExpertSwitchType {
  final String id;
  final String name;
  final List<String> buttons;
  final List<String> actionTypes;
  final Map<String, Object?> defaultMapping;

  const _ExpertSwitchType({
    required this.id,
    required this.name,
    required this.buttons,
    required this.actionTypes,
    required this.defaultMapping,
  });
}

class _ExpertSwitchmapLoad {
  final Map<String, _ExpertSwitchmapType> types;
  final Map<String, Map<String, Object?>> customMappings;
  final Map<String, List<_ExpertSwitchmapAction>> actionCategories;
  final List<_ExpertSwitchmapAction> whenOffOptions;

  const _ExpertSwitchmapLoad({
    required this.types,
    required this.customMappings,
    required this.actionCategories,
    required this.whenOffOptions,
  });

  const _ExpertSwitchmapLoad.empty()
      : types = const {},
        customMappings = const {},
        actionCategories = const {},
        whenOffOptions = const [];

  factory _ExpertSwitchmapLoad.fromModern({
    required Map<dynamic, dynamic> actionsRaw,
    required Map<dynamic, dynamic> switchmapRaw,
  }) {
    final actionCategories = <String, List<_ExpertSwitchmapAction>>{};
    final actions = <_ExpertSwitchmapAction>[];
    final whenOffIds = <String>[];
    final rawActions = actionsRaw['actions'];
    if (rawActions is Iterable) {
      for (final item in rawActions) {
        final body = _asMapOrNull(item);
        if (body == null) continue;
        final action = _ExpertSwitchmapAction.fromModernAction(body);
        actions.add(action);
        final category = _stringValue(body['category']) ?? 'Actions';
        (actionCategories[category] ??= []).add(action);
        for (final id in action.allowedWhenOffValues) {
          if (!whenOffIds.contains(id)) whenOffIds.add(id);
        }
      }
    }
    final actionById = {
      for (final action in actions)
        if (action.id != null) action.id!: action,
    };
    final whenOffOptions = [
      for (final id in whenOffIds)
        actionById[id] ??
            _ExpertSwitchmapAction(
              id: id,
              label: _titleCase(id),
              supportsWhenOff: false,
              allowedWhenOffValues: const [],
            ),
    ];
    return _ExpertSwitchmapLoad(
      types: _modernSwitchmapTypes(switchmapRaw, actionCategories),
      customMappings: _modernSwitchmapCustomMappings(switchmapRaw),
      actionCategories: actionCategories,
      whenOffOptions: whenOffOptions,
    );
  }
}

class _ExpertSwitchmapType {
  final String id;
  final String name;
  final List<String> buttons;
  final List<String> actionTypes;
  final Map<String, Object?> defaultMapping;
  final Map<String, Object?> effectiveMapping;
  final bool hasCustom;

  const _ExpertSwitchmapType({
    required this.id,
    required this.name,
    required this.buttons,
    required this.actionTypes,
    required this.defaultMapping,
    required this.effectiveMapping,
    required this.hasCustom,
  });
}

class _ExpertSwitchmapAction {
  final String? id;
  final String label;
  final bool supportsWhenOff;
  final List<String> allowedWhenOffValues;

  const _ExpertSwitchmapAction({
    required this.id,
    required this.label,
    required this.supportsWhenOff,
    this.allowedWhenOffValues = const [],
  });

  factory _ExpertSwitchmapAction.fromModernAction(Map<dynamic, dynamic> raw) {
    return _ExpertSwitchmapAction(
      id: _stringValue(
        raw['python_action_id'] ?? raw['migrated_id'] ?? raw['rust_action'],
      ),
      label: _stringValue(raw['label']) ??
          _titleCase(_stringValue(raw['rust_action']) ?? 'action'),
      supportsWhenOff: _boolValue(raw['supports_when_off'], fallback: false),
      allowedWhenOffValues: _orderedStringList(raw['allowed_when_off_values']),
    );
  }
}

class _ExpertAreaHistory {
  final List<_ExpertHistoryEntry> entries;
  final String? hint;

  const _ExpertAreaHistory({
    required this.entries,
    required this.hint,
  });
}

class _ExpertHistoryEntry {
  final String title;
  final String? subtitle;
  final String timeLabel;
  final bool isNow;

  const _ExpertHistoryEntry({
    required this.title,
    required this.subtitle,
    required this.timeLabel,
    this.isNow = false,
  });

  factory _ExpertHistoryEntry.fromNow(Map<dynamic, dynamic> raw) {
    final brightness = _nullableDouble(raw['brightness']);
    final kelvin = _nullableDouble(raw['kelvin']);
    final isOn = _boolValue(raw['is_on'], fallback: false);
    final isCircadian = _boolValue(raw['is_circadian'], fallback: false);
    final subtitleParts = <String>[
      isOn ? 'on' : 'off',
      isCircadian ? 'circadian' : 'manual',
      if (brightness != null) '${brightness.round()}%',
      if (kelvin != null) '${kelvin.round()} K',
    ];
    return _ExpertHistoryEntry(
      title: 'Now',
      subtitle: subtitleParts.join(' · '),
      timeLabel: _historyTimeLabel(raw['ts']),
      isNow: true,
    );
  }

  factory _ExpertHistoryEntry.fromActivity(
    _ExpertActivityEntry entry, {
    String? areaName,
  }) {
    final subtitle = _activitySubtitle(entry, areaName);
    return _ExpertHistoryEntry(
      title: _activityActionLabel(entry.action),
      subtitle: subtitle.isEmpty ? null : subtitle,
      timeLabel: _historyTimeLabel(entry.ts),
    );
  }
}

class _SliderPreviewPoint {
  final int brightness;
  final int kelvin;

  const _SliderPreviewPoint({
    required this.brightness,
    required this.kelvin,
  });
}

class _ExpertAreaLightData {
  final List<_ExpertLightRow> rows;
  final List<_ExpertSectionOption> sections;
  final List<_ExpertSectionAdjustRow> sectionAdjusts;
  final List<_ExpertScheduleParticipant> participants;
  final List<_ExpertSectionTuneRow> sectionTunes;
  final List<String> presets;
  final String? feedbackTarget;
  final List<_ExpertFeedbackTargetChoice> feedbackTargetChoices;

  const _ExpertAreaLightData({
    required this.rows,
    required this.sections,
    required this.sectionAdjusts,
    required this.participants,
    required this.sectionTunes,
    required this.presets,
    required this.feedbackTarget,
    required this.feedbackTargetChoices,
  });

  _ExpertAreaLightData copyWith({
    List<_ExpertFeedbackTargetChoice>? feedbackTargetChoices,
  }) {
    return _ExpertAreaLightData(
      rows: rows,
      sections: sections,
      sectionAdjusts: sectionAdjusts,
      participants: participants,
      sectionTunes: sectionTunes,
      presets: presets,
      feedbackTarget: feedbackTarget,
      feedbackTargetChoices:
          feedbackTargetChoices ?? this.feedbackTargetChoices,
    );
  }
}

_ExpertAreaLightData _areaLightDataFromTopology(
  String areaId,
  List<RhythmTopologyNode> topologyNodes,
) {
  final lightNodes = topologyNodes
      .where(
        (node) =>
            node.parentId == areaId &&
            RhythmDeviceType.fromNodeKind(node.kind) == RhythmDeviceType.light,
      )
      .toList()
    ..sort((left, right) => left.name.compareTo(right.name));

  final rows = <_ExpertLightRow>[];
  final sections = <_ExpertSectionOption>[];
  final sectionAdjusts = <_ExpertSectionAdjustRow>[];
  final participants = <_ExpertScheduleParticipant>[];
  final sectionTunes = <_ExpertSectionTuneRow>[];

  for (final node in lightNodes) {
    final name = node.name.isEmpty ? node.id : node.name;
    rows.add(
      _ExpertLightRow(
        entityId: node.id,
        name: name,
        purpose: 'Standard',
        sectionId: node.id,
        sectionName: name,
      ),
    );
    sections.add(_ExpertSectionOption(id: node.id, name: name));
    sectionAdjusts.add(
      _ExpertSectionAdjustRow(
        id: node.id,
        name: name,
        isMain: false,
        isOn: true,
        currentBrightness: null,
        homeBrightness: null,
        brightnessOverride: null,
        autoOffAt: null,
      ),
    );
    participants.add(
      _ExpertScheduleParticipant(
        id: node.id,
        name: name,
        isMain: false,
        participatesInAutoOn: true,
        participatesInAutoOff: true,
      ),
    );
    sectionTunes.add(
      _ExpertSectionTuneRow(
        id: node.id,
        name: name,
        isMain: false,
        balance: null,
        sunDimming: null,
      ),
    );
  }

  return _ExpertAreaLightData(
    rows: rows,
    sections: sections,
    sectionAdjusts: sectionAdjusts,
    participants: participants,
    sectionTunes: sectionTunes,
    presets: const ['Standard'],
    feedbackTarget: null,
    feedbackTargetChoices: _feedbackTargetChoicesFromRows(rows, null),
  );
}

_ExpertAreaLightData? _areaLightDataFromExpertScope(
  String areaId,
  Map<dynamic, dynamic> raw,
) {
  final area = _scopeAreaById(raw, areaId);
  if (area == null) return null;

  final rawSections = area['sections'];
  final sections = <_ExpertSectionOption>[];
  final sectionAdjusts = <_ExpertSectionAdjustRow>[
    _sectionAdjustFromScope(areaId, 'Main', area, isMain: true),
  ];
  final participants = <_ExpertScheduleParticipant>[
    _scheduleParticipantFromScope(areaId, 'Main', area, isMain: true),
  ];
  final sectionTunes = <_ExpertSectionTuneRow>[
    _sectionTuneFromScope(areaId, 'Main', area, isMain: true),
  ];

  if (rawSections is Iterable) {
    for (final rawSection in rawSections) {
      final section = _asMapOrNull(rawSection);
      final sectionId = _stringValue(section?['id']);
      if (section == null || sectionId == null) continue;
      final name = _stringValue(section['name']) ?? sectionId;
      sections.add(_ExpertSectionOption(id: sectionId, name: name));
      sectionAdjusts.add(
        _sectionAdjustFromScope(sectionId, name, section, isMain: false),
      );
      participants.add(
        _scheduleParticipantFromScope(sectionId, name, section, isMain: false),
      );
      sectionTunes.add(
        _sectionTuneFromScope(sectionId, name, section, isMain: false),
      );
    }
  }

  final rows = <_ExpertLightRow>[];
  final rawLights = area['lights'];
  if (rawLights is Iterable) {
    for (final rawLight in rawLights) {
      final light = _asMapOrNull(rawLight);
      final entityId = _stringValue(light?['entity_id']);
      if (entityId == null) continue;
      rows.add(
        _ExpertLightRow(
          entityId: entityId,
          name: _stringValue(light?['name']) ?? entityId,
          purpose: _stringValue(light?['purpose']) ?? 'Standard',
          sectionId: _stringValue(light?['section_id']),
          sectionName: _stringValue(light?['section_name']) ?? 'Main',
        ),
      );
    }
  }

  final presets = _uniqueOrdered([
    'Standard',
    for (final row in rows) row.purpose,
  ]);

  final feedbackTarget = _stringValue(area['feedback_target']);

  return _ExpertAreaLightData(
    rows: rows,
    sections: sections,
    sectionAdjusts: sectionAdjusts,
    participants: participants,
    sectionTunes: sectionTunes,
    presets: presets,
    feedbackTarget: feedbackTarget,
    feedbackTargetChoices: _feedbackTargetChoicesFromRows(
      rows,
      feedbackTarget,
    ),
  );
}

Map<dynamic, dynamic>? _scopeAreaById(
  Map<dynamic, dynamic> raw,
  String areaId,
) {
  final rawZones = raw['zones'];
  if (rawZones is! Iterable) return null;
  for (final rawZone in rawZones) {
    final zone = _asMapOrNull(rawZone);
    final rawAreas = zone?['areas'];
    if (rawAreas is! Iterable) continue;
    for (final rawArea in rawAreas) {
      final area = _asMapOrNull(rawArea);
      if (_stringValue(area?['id']) == areaId) return area;
    }
  }
  return null;
}

_ExpertSectionAdjustRow _sectionAdjustFromScope(
  String id,
  String name,
  Map<dynamic, dynamic> raw, {
  required bool isMain,
}) {
  final effective = _asMapOrNull(raw['effective']);
  final layer = _asMapOrNull(raw['layer']);
  final brightness = isMain
      ? _scopeBrightnessOverride(layer) ?? _scopeBrightnessOverride(effective)
      : _scopeBrightnessTrim(layer) ?? _scopeBrightnessTrim(effective);
  return _ExpertSectionAdjustRow(
    id: id,
    name: name,
    isMain: isMain,
    isOn: !_boolValue(effective?['hard_off'], fallback: false),
    currentBrightness: isMain ? brightness : null,
    homeBrightness: null,
    brightnessOverride: brightness,
    autoOffAt: _stringValue(_asMapOrNull(effective?['auto_off'])?['deadline']),
  );
}

_ExpertScheduleParticipant _scheduleParticipantFromScope(
  String id,
  String name,
  Map<dynamic, dynamic> raw, {
  required bool isMain,
}) {
  final effective = _asMapOrNull(raw['effective']);
  return _ExpertScheduleParticipant(
    id: id,
    name: name,
    isMain: isMain,
    participatesInAutoOn: _boolValue(
      effective?['auto_on_participation'],
      fallback: true,
    ),
    participatesInAutoOff: _boolValue(
      effective?['auto_off_participation'],
      fallback: true,
    ),
  );
}

_ExpertSectionTuneRow _sectionTuneFromScope(
  String id,
  String name,
  Map<dynamic, dynamic> raw, {
  required bool isMain,
}) {
  final layer = _asMapOrNull(raw['layer']);
  return _ExpertSectionTuneRow(
    id: id,
    name: name,
    isMain: isMain,
    balance: _nullableDouble(layer?['balance']),
    sunDimming: _nullableDouble(layer?['local_sun_exposure']),
  );
}

class _ExpertFeedbackTargetChoice {
  final String value;
  final String label;
  final String detail;

  const _ExpertFeedbackTargetChoice({
    required this.value,
    required this.label,
    required this.detail,
  });
}

List<_ExpertFeedbackTargetChoice> _feedbackTargetChoicesFromRows(
  List<_ExpertLightRow> rows,
  String? currentTarget,
) {
  final purposeCounts = <String, int>{};
  for (final row in rows) {
    purposeCounts[row.purpose] = (purposeCounts[row.purpose] ?? 0) + 1;
  }
  return _feedbackTargetChoicesFromCountsAndLights(
    purposeCounts: purposeCounts,
    lights: [
      for (final row in rows)
        _ExpertFeedbackTargetChoice(
          value: row.entityId,
          label: row.name,
          detail: row.entityId,
        ),
    ],
    currentTarget: currentTarget,
  );
}

List<_ExpertFeedbackTargetChoice> _feedbackTargetChoicesFromCountsAndLights({
  required Map<String, int> purposeCounts,
  required List<_ExpertFeedbackTargetChoice> lights,
  required String? currentTarget,
}) {
  final choices = <_ExpertFeedbackTargetChoice>[
    const _ExpertFeedbackTargetChoice(
      value: '',
      label: 'Auto',
      detail: 'Most populated purpose',
    ),
  ];
  final purposeEntries = purposeCounts.entries.toList()
    ..sort((a, b) {
      final countCompare = b.value.compareTo(a.value);
      if (countCompare != 0) return countCompare;
      if (a.key == 'Standard') return -1;
      if (b.key == 'Standard') return 1;
      return a.key.compareTo(b.key);
    });
  for (final entry in purposeEntries) {
    final count = entry.value;
    choices.add(
      _ExpertFeedbackTargetChoice(
        value: entry.key,
        label: '${entry.key} ($count)',
        detail: 'Purpose',
      ),
    );
  }
  choices.addAll(lights);

  if (currentTarget != null &&
      currentTarget.isNotEmpty &&
      !choices.any((choice) => choice.value == currentTarget)) {
    choices.add(
      _ExpertFeedbackTargetChoice(
        value: currentTarget,
        label: currentTarget.startsWith('light.')
            ? _entityName(currentTarget)
            : currentTarget,
        detail: 'Saved target',
      ),
    );
  }
  return choices;
}

class _ExpertSectionOption {
  final String id;
  final String name;

  const _ExpertSectionOption({
    required this.id,
    required this.name,
  });
}

class _ExpertReachSection {
  final String id;
  final String name;
  final String areaId;
  final String areaName;

  const _ExpertReachSection({
    required this.id,
    required this.name,
    required this.areaId,
    required this.areaName,
  });

  String get label => '$areaName · $name';
}

class _ExpertSectionAdjustRow {
  final String id;
  final String name;
  final bool isMain;
  final bool isOn;
  final double? currentBrightness;
  final double? homeBrightness;
  final double? brightnessOverride;
  final String? autoOffAt;

  const _ExpertSectionAdjustRow({
    required this.id,
    required this.name,
    required this.isMain,
    required this.isOn,
    required this.currentBrightness,
    required this.homeBrightness,
    required this.brightnessOverride,
    required this.autoOffAt,
  });

  bool get hasBrightnessOverride => brightnessOverride != null;
}

class _ExpertLightRow {
  final String entityId;
  final String name;
  final String purpose;
  final String? sectionId;
  final String sectionName;

  const _ExpertLightRow({
    required this.entityId,
    required this.name,
    required this.purpose,
    required this.sectionId,
    required this.sectionName,
  });

  _ExpertLightRow copyWith({
    String? purpose,
    String? sectionId,
    String? sectionName,
    bool clearSection = false,
  }) {
    return _ExpertLightRow(
      entityId: entityId,
      name: name,
      purpose: purpose ?? this.purpose,
      sectionId: clearSection ? null : (sectionId ?? this.sectionId),
      sectionName: sectionName ?? this.sectionName,
    );
  }
}

class _ExpertLightGroup {
  final String name;
  final List<_ExpertLightRow> rows;

  const _ExpertLightGroup({
    required this.name,
    required this.rows,
  });
}

class _ExpertScheduleParticipant {
  final String id;
  final String name;
  final bool isMain;
  final bool participatesInAutoOn;
  final bool participatesInAutoOff;

  const _ExpertScheduleParticipant({
    required this.id,
    required this.name,
    required this.isMain,
    required this.participatesInAutoOn,
    required this.participatesInAutoOff,
  });

  _ExpertScheduleParticipant copyWith({
    bool? participatesInAutoOn,
    bool? participatesInAutoOff,
  }) {
    return _ExpertScheduleParticipant(
      id: id,
      name: name,
      isMain: isMain,
      participatesInAutoOn: participatesInAutoOn ?? this.participatesInAutoOn,
      participatesInAutoOff:
          participatesInAutoOff ?? this.participatesInAutoOff,
    );
  }
}

class _ExpertSectionTuneRow {
  final String id;
  final String name;
  final bool isMain;
  final double? balance;
  final double? sunDimming;

  const _ExpertSectionTuneRow({
    required this.id,
    required this.name,
    required this.isMain,
    required this.balance,
    required this.sunDimming,
  });

  _ExpertSectionTuneRow copyWith({
    double? balance,
    double? sunDimming,
    bool clearBalance = false,
    bool clearSunDimming = false,
  }) {
    return _ExpertSectionTuneRow(
      id: id,
      name: name,
      isMain: isMain,
      balance: clearBalance ? null : (balance ?? this.balance),
      sunDimming: clearSunDimming ? null : (sunDimming ?? this.sunDimming),
    );
  }
}

class _ExpertAreaSettings {
  final Map<String, Object?> raw;

  const _ExpertAreaSettings._(this.raw);

  factory _ExpertAreaSettings.defaults() {
    return _ExpertAreaSettings.fromServer(const {});
  }

  factory _ExpertAreaSettings.fromServer(Map<dynamic, dynamic> raw) {
    return _ExpertAreaSettings._({
      'motion_function': _stringValue(raw['motion_function']) ?? 'disabled',
      'motion_duration':
          _intValue(raw['motion_duration'], fallback: 60).clamp(15, 240),
      'auto_on_enabled': _boolValue(raw['auto_on_enabled'], fallback: false),
      'auto_on_source': _stringValue(raw['auto_on_source']) ?? 'sunset',
      'auto_on_offset': _intValue(raw['auto_on_offset'], fallback: 0),
      'auto_on_days': _intListValue(
        raw['auto_on_days'],
        fallback: _allWeekdays,
      ),
      'auto_on_time_1': _nullableDouble(raw['auto_on_time_1']),
      'auto_on_days_1': _intListValue(
        raw['auto_on_days_1'],
        fallback: const [],
      ),
      'auto_on_time_2': _nullableDouble(raw['auto_on_time_2']),
      'auto_on_days_2': _intListValue(
        raw['auto_on_days_2'],
        fallback: const [],
      ),
      'auto_on_fade': _intValue(raw['auto_on_fade'], fallback: 0),
      'auto_on_skip_if_brighter':
          _boolValue(raw['auto_on_skip_if_brighter'], fallback: false),
      'auto_on_trigger_mode':
          _stringValue(raw['auto_on_trigger_mode']) ?? 'always',
      'auto_on_light': _stringValue(raw['auto_on_light']) ?? 'circadian',
      'auto_on_override': _asMapOrNull(raw['auto_on_override']),
      'auto_off_enabled': _boolValue(raw['auto_off_enabled'], fallback: false),
      'auto_off_source': _stringValue(raw['auto_off_source']) ?? 'sunrise',
      'auto_off_offset': _intValue(raw['auto_off_offset'], fallback: 0),
      'auto_off_days': _intListValue(
        raw['auto_off_days'],
        fallback: _allWeekdays,
      ),
      'auto_off_time_1': _nullableDouble(raw['auto_off_time_1']),
      'auto_off_days_1': _intListValue(
        raw['auto_off_days_1'],
        fallback: const [],
      ),
      'auto_off_time_2': _nullableDouble(raw['auto_off_time_2']),
      'auto_off_days_2': _intListValue(
        raw['auto_off_days_2'],
        fallback: const [],
      ),
      'auto_off_fade': _intValue(raw['auto_off_fade'], fallback: 0),
      'auto_off_only_untouched':
          _boolValue(raw['auto_off_only_untouched'], fallback: false),
      'auto_off_override': _asMapOrNull(raw['auto_off_override']),
    });
  }

  _ExpertAreaSettings merged(Map<String, Object?> updates) {
    return _ExpertAreaSettings.fromServer({...raw, ...updates});
  }

  String get motionFunction =>
      _stringValue(raw['motion_function']) ?? 'disabled';
  int get motionDuration => _intValue(raw['motion_duration'], fallback: 60);
  bool get autoOnEnabled => _boolValue(raw['auto_on_enabled'], fallback: false);
  String get autoOnSource => _stringValue(raw['auto_on_source']) ?? 'sunset';
  int get autoOnOffset => _intValue(raw['auto_on_offset'], fallback: 0);
  List<int> get autoOnDays =>
      _intListValue(raw['auto_on_days'], fallback: _allWeekdays);
  double? get autoOnTime1 => _nullableDouble(raw['auto_on_time_1']);
  List<int> get autoOnDays2 =>
      _intListValue(raw['auto_on_days_2'], fallback: const []);
  double? get autoOnTime2 => _nullableDouble(raw['auto_on_time_2']);
  int get autoOnFade => _intValue(raw['auto_on_fade'], fallback: 0);
  bool get autoOnSkipIfBrighter =>
      _boolValue(raw['auto_on_skip_if_brighter'], fallback: false);
  String get autoOnTriggerMode =>
      _stringValue(raw['auto_on_trigger_mode']) ??
      (autoOnSkipIfBrighter ? 'skip_brighter' : 'always');
  String get autoOnLight => _stringValue(raw['auto_on_light']) ?? 'circadian';
  _ExpertAutoScheduleOverride? get autoOnOverride =>
      _ExpertAutoScheduleOverride.fromServer(raw['auto_on_override']);
  bool get autoOffEnabled =>
      _boolValue(raw['auto_off_enabled'], fallback: false);
  String get autoOffSource => _stringValue(raw['auto_off_source']) ?? 'sunrise';
  int get autoOffOffset => _intValue(raw['auto_off_offset'], fallback: 0);
  List<int> get autoOffDays =>
      _intListValue(raw['auto_off_days'], fallback: _allWeekdays);
  double? get autoOffTime1 => _nullableDouble(raw['auto_off_time_1']);
  List<int> get autoOffDays2 =>
      _intListValue(raw['auto_off_days_2'], fallback: const []);
  double? get autoOffTime2 => _nullableDouble(raw['auto_off_time_2']);
  int get autoOffFade => _intValue(raw['auto_off_fade'], fallback: 0);
  bool get autoOffOnlyUntouched =>
      _boolValue(raw['auto_off_only_untouched'], fallback: false);
  _ExpertAutoScheduleOverride? get autoOffOverride =>
      _ExpertAutoScheduleOverride.fromServer(raw['auto_off_override']);
}

class _ExpertAutoScheduleOverride {
  final Map<dynamic, dynamic> raw;

  const _ExpertAutoScheduleOverride._(this.raw);

  static _ExpertAutoScheduleOverride? fromServer(Object? raw) {
    final body = _asMapOrNull(raw);
    if (body == null || body.isEmpty) return null;
    return _ExpertAutoScheduleOverride._(body);
  }

  String get mode => _stringValue(raw['mode']) ?? 'pause';

  String? get untilDate => _stringValue(raw['until_date']);

  double? get time => _nullableDouble(raw['time']);

  bool get isPause => mode == 'pause';

  String get label {
    final through = untilDate == null ? '' : ' through $untilDate';
    if (isPause) return 'Paused$through';
    final display = time == null ? 'custom time' : _formatHour(time!);
    return '$display$through';
  }
}

class _RhythmDefinition {
  final String sleepPattern;
  final double wakeHour;
  final double bedHour;
  final int minBrightness;
  final int maxBrightness;
  final int sleepBrightness;
  final int minKelvin;
  final int maxKelvin;
  final double daylightDimming;
  final double naturalExposure;
  final double transitionMinutes;
  final double phaseBalance;

  const _RhythmDefinition({
    required this.sleepPattern,
    required this.wakeHour,
    required this.bedHour,
    required this.minBrightness,
    required this.maxBrightness,
    required this.sleepBrightness,
    required this.minKelvin,
    required this.maxKelvin,
    required this.daylightDimming,
    required this.naturalExposure,
    required this.transitionMinutes,
    required this.phaseBalance,
  });

  const _RhythmDefinition.defaults()
      : sleepPattern = 'adult',
        wakeHour = 7.0,
        bedHour = 21.0,
        minBrightness = 18,
        maxBrightness = 92,
        sleepBrightness = 8,
        minKelvin = 2200,
        maxKelvin = 5200,
        daylightDimming = 0.35,
        naturalExposure = 0.55,
        transitionMinutes = 30,
        phaseBalance = 0;

  _RhythmDefinition copyWith({
    String? sleepPattern,
    double? wakeHour,
    double? bedHour,
    int? minBrightness,
    int? maxBrightness,
    int? sleepBrightness,
    int? minKelvin,
    int? maxKelvin,
    double? daylightDimming,
    double? naturalExposure,
    double? transitionMinutes,
    double? phaseBalance,
  }) {
    return _RhythmDefinition(
      sleepPattern: sleepPattern ?? this.sleepPattern,
      wakeHour: wakeHour ?? this.wakeHour,
      bedHour: bedHour ?? this.bedHour,
      minBrightness: minBrightness ?? this.minBrightness,
      maxBrightness: maxBrightness ?? this.maxBrightness,
      sleepBrightness: sleepBrightness ?? this.sleepBrightness,
      minKelvin: minKelvin ?? this.minKelvin,
      maxKelvin: maxKelvin ?? this.maxKelvin,
      daylightDimming: daylightDimming ?? this.daylightDimming,
      naturalExposure: naturalExposure ?? this.naturalExposure,
      transitionMinutes: transitionMinutes ?? this.transitionMinutes,
      phaseBalance: phaseBalance ?? this.phaseBalance,
    );
  }

  @override
  bool operator ==(Object other) {
    return other is _RhythmDefinition &&
        other.sleepPattern == sleepPattern &&
        other.wakeHour == wakeHour &&
        other.bedHour == bedHour &&
        other.minBrightness == minBrightness &&
        other.maxBrightness == maxBrightness &&
        other.sleepBrightness == sleepBrightness &&
        other.minKelvin == minKelvin &&
        other.maxKelvin == maxKelvin &&
        other.daylightDimming == daylightDimming &&
        other.naturalExposure == naturalExposure &&
        other.transitionMinutes == transitionMinutes &&
        other.phaseBalance == phaseBalance;
  }

  @override
  int get hashCode => Object.hash(
        sleepPattern,
        wakeHour,
        bedHour,
        minBrightness,
        maxBrightness,
        sleepBrightness,
        minKelvin,
        maxKelvin,
        daylightDimming,
        naturalExposure,
        transitionMinutes,
        phaseBalance,
      );
}

class _CurveReading {
  final int brightness;
  final int kelvin;

  const _CurveReading({
    required this.brightness,
    required this.kelvin,
  });

  factory _CurveReading.from(_RhythmDefinition definition, double hour) {
    final normalized = _normalizeHour(hour);
    final peak = 13 + definition.phaseBalance * 1.5;
    final dayShape = _dayShape(normalized, peak);
    final inSleep = _isInSleepWindow(
      normalized,
      definition.bedHour,
      definition.wakeHour,
    );
    final daylightTrim =
        dayShape * definition.daylightDimming * definition.naturalExposure * 18;

    final rawBrightness = inSleep
        ? definition.sleepBrightness + dayShape * 8
        : definition.minBrightness +
            (definition.maxBrightness - definition.minBrightness) *
                math.pow(dayShape, 1.15) -
            daylightTrim;
    final rawKelvin = definition.minKelvin +
        (definition.maxKelvin - definition.minKelvin) *
            math.pow(dayShape, 0.85);

    return _CurveReading(
      brightness: rawBrightness.round().clamp(0, 100),
      kelvin: rawKelvin.round().clamp(1500, 7000),
    );
  }
}

class _CircadianExpertClient {
  final Dio _dio;
  final ServerSyncProvider _syncProvider;
  final Map<String, List<String>> _zoneAreaIds = {};
  final Map<String, String> _zoneIdsByName = {};
  final Map<String, String> _areaNamesById = {};
  String? _defaultZoneId;
  final String baseUrlLabel;

  _CircadianExpertClient._({
    required String baseUrl,
    required String? authToken,
    required ServerSyncProvider syncProvider,
  })  : baseUrlLabel = _trimTrailingSlash(baseUrl),
        _syncProvider = syncProvider,
        _dio = Dio(
          BaseOptions(
            baseUrl: _normalizeBaseUrl(baseUrl),
            connectTimeout: const Duration(seconds: 3),
            receiveTimeout: const Duration(seconds: 5),
            headers: _bearerHeaders(authToken),
          ),
        );

  static Future<_CircadianExpertClient?> resolve(BuildContext context) async {
    final homeProvider = context.read<HomeProvider>();
    final syncProvider = context.read<ServerSyncProvider>();
    final hub = homeProvider.activeServerHub;
    if (hub == null) return null;

    final resolved = await ServerEndpointResolver.resolve(
      hub,
      syncProvider: syncProvider,
    );
    return _CircadianExpertClient._(
      baseUrl: resolved.baseUrl,
      authToken: resolved.hub.token,
      syncProvider: syncProvider,
    );
  }

  Future<_ZoneLoadResult> fetchZones() async {
    final cached = _zonesFromModernRooms(_syncProvider.helloRooms);
    try {
      final fresh = await _fetchModernZones();
      if (fresh.visibleZones.isNotEmpty) return _rememberZoneAreas(fresh);
    } catch (error, stackTrace) {
      if (cached.visibleZones.isNotEmpty) return _rememberZoneAreas(cached);
      debugPrint('CircadianExpert: modern topology load failed: $error');
      debugPrint('$stackTrace');
      rethrow;
    }
    return _rememberZoneAreas(cached);
  }

  _ZoneLoadResult _rememberZoneAreas(_ZoneLoadResult result) {
    _zoneAreaIds
      ..clear()
      ..addEntries(
        result.visibleZones.map(
          (zone) => MapEntry(
            zone.name,
            zone.areas.map((area) => area.id).toList(growable: false),
          ),
        ),
      );
    _zoneIdsByName
      ..clear()
      ..addEntries(
        result.visibleZones.map((zone) => MapEntry(zone.name, zone.id)),
      );
    _areaNamesById
      ..clear()
      ..addEntries(
        result.visibleZones.expand(
          (zone) => zone.areas.map((area) => MapEntry(area.id, area.name)),
        ),
      );
    _defaultZoneId = result.visibleZones
        .where((zone) => zone.isDefault)
        .map((zone) => zone.id)
        .firstOrNull;
    return result;
  }

  String _zoneIdFor(String nameOrId) => _zoneIdsByName[nameOrId] ?? nameOrId;

  String _runtimeZoneIdFor(String nameOrId) {
    final mapped = _zoneIdsByName[nameOrId];
    if (mapped != null) return mapped;
    if (nameOrId == 'Rhythm Topology' && _defaultZoneId != null) {
      return _defaultZoneId!;
    }
    return nameOrId;
  }

  Future<Map<dynamic, dynamic>> _fetchExpertProfileConfig() async {
    final response = await _dio.get('api/config');
    return _asMap(response.data);
  }

  Future<_ZoneLoadResult> _fetchModernZones() async {
    try {
      final scopeResponse = await _dio.get('$_removed-projectRuntimeApi/scope');
      final config = await _fetchExpertProfileConfig();
      return _zonesFromExpertScope(
        _asMap(scopeResponse.data),
        _rhythmDefinitionFromExpertProfile(config),
      );
    } on DioException catch (error) {
      if (error.response?.statusCode != 404) rethrow;
    }

    final responses = await Future.wait([
      _dio.get('api/state'),
      _dio.get('api/topology/nodes'),
    ]);
    final hello = RhythmHello.fromJson(_stringMap(responses[0].data));
    final topologyNodes = _topologyNodesFromPayload(responses[1].data);
    return _zonesFromModernRooms(
      _combinedModernRooms(
        stateNodes: hello.nodes,
        topologyNodes: topologyNodes,
      ),
    );
  }

  Future<_ExpertServerSnapshot> fetchServerSnapshot() async {
    Map<dynamic, dynamic>? state;
    Map<dynamic, dynamic>? now;

    try {
      final response = await _dio.get('api/state');
      state = _asMap(response.data);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: server state load failed: $error');
      debugPrint('$stackTrace');
    }

    try {
      final activeProfile = _asMapOrNull(state?['active_profile']);
      final activeProfileId = _stringValue(activeProfile?['id']) ?? 'rhythm';
      final response = await _dio.get(
        'api/curve/now',
        queryParameters: {'id': activeProfileId},
      );
      now = _asMap(response.data);
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: curve-now load failed: $error');
      debugPrint('$stackTrace');
    }

    return _ExpertServerSnapshot.fromModern(
      state: state,
      now: now,
    );
  }

  Future<void> createZone(String name) async {
    await _dio.post('api/light-runtimes/removed-circadian/zones',
        data: {'name': name});
  }

  Future<void> renameZone(String oldName, String newName) async {
    await _dio.put(
      'api/light-runtimes/removed-circadian/zones/${Uri.encodeComponent(_zoneIdFor(oldName))}',
      data: {'name': newName},
    );
  }

  Future<void> setDefaultZone(String name) async {
    await _dio.put(
      'api/light-runtimes/removed-circadian/zones/${Uri.encodeComponent(_zoneIdFor(name))}/default',
    );
  }

  Future<void> deleteZone(String name) async {
    await _dio.delete(
      'api/light-runtimes/removed-circadian/zones/${Uri.encodeComponent(_zoneIdFor(name))}',
    );
  }

  Future<void> reorderZones(List<String> order) async {
    await _dio.put(
      'api/light-runtimes/removed-circadian/zones/reorder',
      data: {'order': order.map(_zoneIdFor).toList(growable: false)},
    );
  }

  Future<void> reorderZoneAreas(
    String zoneName,
    Iterable<String> areaIds,
  ) async {
    await _dio.put(
      'api/light-runtimes/removed-circadian/zones/${Uri.encodeComponent(_zoneIdFor(zoneName))}/areas',
      data: {'area_ids': areaIds.toList(growable: false)},
    );
  }

  Future<void> moveAreaToZone(String zoneName, _ExpertArea area) async {
    await _dio.put(
      'api/light-runtimes/removed-circadian/areas/${Uri.encodeComponent(area.id)}/zone',
      data: {'zone_id': _zoneIdFor(zoneName)},
    );
  }

  Future<void> removeAreaFromZone(String zoneName, String areaId) async {
    final targetZoneId = _defaultZoneId;
    if (targetZoneId == null || targetZoneId == _zoneIdFor(zoneName)) return;
    await _dio.put(
      'api/light-runtimes/removed-circadian/areas/${Uri.encodeComponent(areaId)}/zone',
      data: {'zone_id': targetZoneId},
    );
  }

  Future<void> purgeAreaFromConfig(String areaId) async {
    await _dio.delete(
        'api/light-runtimes/removed-circadian/areas/${Uri.encodeComponent(areaId)}');
  }

  Future<void> updateZoneSettings(
    String name,
    _RhythmDefinition definition,
  ) async {
    await _dio.put(
      '$_removed-projectRuntimeApi/zones/${Uri.encodeComponent(_zoneIdFor(name))}/rhythm',
      data: _expertProfileConfigFromDefinition(definition),
    );
  }

  Future<_RhythmDefinition> fetchExpertRhythmDefinition(String zoneName) async {
    final response = await _dio.get(
      '$_removed-projectRuntimeApi/zones/${Uri.encodeComponent(_zoneIdFor(zoneName))}/rhythm',
    );
    return _rhythmDefinitionFromExpertProfile(_asMap(response.data));
  }

  Future<_ExpertZoneSchedule> fetchZoneSchedule(String zoneName) async {
    try {
      final response = await _dio.get(
        'api/light-runtimes/removed-circadian/zones/${Uri.encodeComponent(_zoneIdFor(zoneName))}/schedule',
      );
      final body = _asMap(response.data);
      return _ExpertZoneSchedule(
        overrideData: _asMapOrNull(body['override']),
        nextTimes: _asMapOrNull(body['next_times']),
      );
    } on DioException catch (error) {
      if (error.response?.statusCode != 404) rethrow;
      return const _ExpertZoneSchedule.unavailable(
        'This build is missing the Expert zone schedule API.',
      );
    }
  }

  Future<_ExpertRhythmPresetLoad> fetchRhythmPresets() async {
    final response = await _dio.get('api/profiles');
    return _ExpertRhythmPresetLoad.fromProfiles(_asMap(response.data));
  }

  Future<_ExpertSunTimes> fetchSunTimes({DateTime? date}) async {
    final response = await _dio.get(
      'api/curve/solar',
      queryParameters: {
        if (date != null) 'date': _isoDate(date),
      },
    );
    return _ExpertSunTimes.fromServer(_asMap(response.data));
  }

  Future<_ExpertCurveData> fetchCurveData(_RhythmDefinition definition) async {
    final base = await _fetchExpertProfileConfig();
    final profileId = _stringValue(base['id']) ?? 'rhythm';
    final config = _expertProfileConfigFromDefinition(definition, base: base)
      ..['id'] = profileId;
    final response = await _dio.post(
      'api/curve',
      queryParameters: {
        'id': profileId,
        'samples_per_hour': 4,
      },
      data: config,
    );
    return _ExpertCurveData.fromServer(_asMap(response.data));
  }

  Future<_ExpertStepSequences> fetchStepSequences(
    _RhythmDefinition definition, {
    required double hour,
    int maxSteps = 8,
  }) async {
    final base = await _fetchExpertProfileConfig();
    final profileId = _stringValue(base['id']) ?? 'rhythm';
    final config = _expertProfileConfigFromDefinition(definition, base: base)
      ..['id'] = profileId;
    final response = await _dio.post(
      'api/curve',
      queryParameters: {
        'id': profileId,
        'start_hour': _roundHundredth(hour),
        'max_steps': maxSteps,
      },
      data: config,
    );
    return _ExpertStepSequences.fromServer(_asMap(response.data));
  }

  Future<void> setZoneScheduleOverride(
    String zoneName, {
    required String mode,
    double? customWake,
    double? customBed,
  }) async {
    await _dio.put(
      'api/light-runtimes/removed-circadian/zones/${Uri.encodeComponent(_zoneIdFor(zoneName))}/schedule',
      data: {
        'mode': mode,
        if (customWake != null) 'custom_wake': _normalizeHour(customWake),
        if (customBed != null) 'custom_bed': _normalizeHour(customBed),
      },
    );
  }

  Future<void> clearZoneScheduleOverride(String zoneName) async {
    await _dio.delete(
      'api/light-runtimes/removed-circadian/zones/${Uri.encodeComponent(_zoneIdFor(zoneName))}/schedule',
    );
  }

  Future<void> setCircadianModeForAreas(
    Iterable<String> areaIds, {
    required bool enabled,
  }) async {
    final nodeIds = areaIds.where((id) => id.trim().isNotEmpty).toList();
    for (final nodeId in nodeIds) {
      await runAreaAction(
        nodeId,
        'set_circadian_enabled',
        {'enabled': enabled},
      );
    }
  }

  Future<void> sendLiveDesignHeartbeat(Iterable<String> areaIds) async {
    final nodeIds = areaIds.where((id) => id.trim().isNotEmpty).toList();
    if (nodeIds.isEmpty) return;
    await _dio.post(
      'api/light-runtimes/removed-circadian/live-design/heartbeat',
      data: {'area_ids': nodeIds},
    );
  }

  Future<void> endLiveDesign(Iterable<String> areaIds) async {
    final nodeIds = areaIds.where((id) => id.trim().isNotEmpty).toList();
    if (nodeIds.isEmpty) return;
    await _dio.post(
      'api/light-runtimes/removed-circadian/live-design/end',
      data: {'area_ids': nodeIds},
    );
  }

  Future<void> applyLightToAreas(
    Iterable<String> areaIds, {
    required int brightness,
    required int kelvin,
    required double transitionSeconds,
  }) async {
    final nodeIds = areaIds.where((id) => id.trim().isNotEmpty).toList();
    if (nodeIds.isEmpty) return;
    final clampedBrightness = brightness.clamp(1, 100).toInt();
    final clampedKelvin = kelvin.clamp(1500, 7000).toInt();
    for (final areaId in nodeIds) {
      await runAreaAction(
        areaId,
        'set_brightness',
        {'brightness': clampedBrightness},
      );
      await runAreaAction(
        areaId,
        'set_color_temperature',
        {'kelvin': clampedKelvin},
      );
    }
  }

  Future<Map<dynamic, dynamic>?> _fetchAreaNow(String areaId) async {
    final response = await _dio.get(
      _expertAreaNowPath(areaId),
    );
    return _asMapOrNull(_asMap(response.data)['node']);
  }

  Future<_ExpertAreaStatus?> fetchAreaStatus(String areaId) async {
    try {
      final node = await _fetchAreaNow(areaId);
      if (node == null) return null;
      var status = _areaStatusFromModernNode(node);
      try {
        final scope =
            await _dio.get('api/light-runtimes/removed-circadian/scope');
        status = _areaStatusWithExpertScope(
          status,
          _asMap(scope.data),
          areaId,
        );
      } on DioException catch (error) {
        if (error.response?.statusCode != 404) rethrow;
      }
      return status;
    } catch (error, stackTrace) {
      debugPrint('CircadianExpert: node status load failed: $error');
      debugPrint('$stackTrace');
      return null;
    }
  }

  Future<_ExpertAreaSettings> fetchAreaSettings(String areaId) async {
    try {
      final response = await _dio.get(
        'api/light-runtimes/removed-circadian/areas/${Uri.encodeComponent(areaId)}/settings',
      );
      return _ExpertAreaSettings.fromServer(_asMap(response.data));
    } on DioException catch (error) {
      if (error.response?.statusCode != 404) rethrow;
      return _ExpertAreaSettings.defaults();
    }
  }

  Future<_ExpertAreaHistory> fetchAreaHistory(
    String areaId, {
    int limit = 12,
  }) async {
    final feed = await fetchActivity(area: areaId, limit: math.max(limit, 200));
    final entries = feed.entries
        .where((entry) => entry.areaId == areaId)
        .toList()
      ..sort((a, b) => b.ts.compareTo(a.ts));
    final history = [
      for (final entry in entries.take(limit))
        _ExpertHistoryEntry.fromActivity(entry),
    ];
    return _ExpertAreaHistory(
      entries: history,
      hint: history.isEmpty ? 'No recent activity.' : null,
    );
  }

  Future<_ExpertAreaStatus> fetchZoneAdjust(String zoneName) async {
    final statuses = <_ExpertAreaStatus>[];
    for (final areaId in _zoneAreaIds[zoneName] ?? const <String>[]) {
      final status = await fetchAreaStatus(areaId);
      if (status != null) statuses.add(status);
    }
    return _aggregateModernStatuses(statuses);
  }

  Future<_ExpertAreaHistory> fetchZoneHistory(
    String zoneName, {
    int limit = 12,
  }) async {
    final areaIds = _zoneAreaIds[zoneName] ?? const <String>[];
    final targetIds = {
      _zoneIdFor(zoneName),
      ...areaIds,
    };
    final feed = await fetchActivity(limit: math.max(limit, 300));
    final entries = feed.entries
        .where(
            (entry) => entry.areaId != null && targetIds.contains(entry.areaId))
        .toList()
      ..sort((a, b) => b.ts.compareTo(a.ts));
    final history = [
      for (final entry in entries.take(limit))
        _ExpertHistoryEntry.fromActivity(
          entry,
          areaName: entry.areaId == null ? null : _areaNamesById[entry.areaId],
        ),
    ];
    return _ExpertAreaHistory(
      entries: history,
      hint: history.isEmpty ? 'No recent activity.' : null,
    );
  }

  Future<_ExpertHistoryEntry?> fetchAreaNow(String areaId) async {
    final node = await _fetchAreaNow(areaId);
    if (node == null) return null;
    return _ExpertHistoryEntry.fromNow(
      _modernNowEntryFromNode(node),
    );
  }

  Future<List<_SliderPreviewPoint>> fetchAreaSliderPreview(
    String areaId, {
    int points = 10,
  }) async {
    final response = await _dio.get(
      'api/light-runtimes/removed-circadian/areas/${Uri.encodeComponent(areaId)}/slider-preview',
      queryParameters: {'points': points.clamp(3, 20)},
    );
    final rawPoints = _asMap(response.data)['points'];
    if (rawPoints is! Iterable) return const [];
    return [
      for (final rawPoint in rawPoints)
        if (rawPoint is Map)
          _SliderPreviewPoint(
            brightness: _nullableInt(rawPoint['brightness']) ?? 0,
            kelvin: _nullableInt(rawPoint['kelvin']) ?? 0,
          ),
    ].where((point) => point.brightness > 0 && point.kelvin > 0).toList();
  }

  Future<_ExpertAreaLightData> fetchAreaLightData(String areaId) async {
    try {
      final response =
          await _dio.get('api/light-runtimes/removed-circadian/scope');
      final expert = _areaLightDataFromExpertScope(
        areaId,
        _asMap(response.data),
      );
      if (expert != null) return expert;
    } on DioException catch (error) {
      if (error.response?.statusCode != 404) rethrow;
    }

    final response = await _dio.get('api/topology/nodes');
    return _areaLightDataFromTopology(
      areaId,
      _topologyNodesFromPayload(response.data),
    );
  }

  Future<_ExpertActivityFeed> fetchActivity({
    String? area,
    String? source,
    String? action,
    int limit = 5000,
  }) async {
    final response = await _dio.get(
      'api/history',
      queryParameters: {
        'limit': limit.clamp(1, 5000),
        if (area != null) 'area': area,
        if (source != null) 'source': source,
        if (action != null) 'action': action,
      },
    );
    return _ExpertActivityFeed.fromServer(_asMap(response.data));
  }

  Future<List<_ExpertActivityEntry>> fetchControlActivity(
    _ExpertControl control,
  ) async {
    final feed = await fetchActivity(limit: 300);
    final ids = {
      control.id,
      if (control.deviceId != null) control.deviceId!,
    };
    return [
      for (final entry in feed.entries)
        if (entry.sourceEntity != null && ids.contains(entry.sourceEntity))
          entry,
    ];
  }

  Future<_ExpertZhaSettings> fetchZhaSettings(String deviceId) async {
    final response = await _dio.get(
      'api/controls/${Uri.encodeComponent(deviceId)}/hardware-settings',
    );
    return _ExpertZhaSettings.fromServer(_asMap(response.data));
  }

  Future<void> saveZhaSettings(
    String deviceId, {
    String? sensitivity,
    int? timeoutSeconds,
  }) async {
    await _dio.put(
      'api/controls/${Uri.encodeComponent(deviceId)}/hardware-settings',
      data: {
        if (sensitivity != null) 'sensitivity': sensitivity,
        if (timeoutSeconds != null) 'occupancy_timeout_secs': timeoutSeconds,
      },
    );
  }

  Future<_ExpertConfig> fetchExpertConfig() async {
    final values = _expertConfigValuesFromServer(
      await _fetchExpertProfileConfig(),
    );

    try {
      final settingsResponse = await _dio.get('api/settings');
      values.addAll(
        _expertSettingsValuesFromServer(_asMap(settingsResponse.data)),
      );
    } on DioException catch (error) {
      if (error.response?.statusCode != 404) rethrow;
    }

    return _ExpertConfig(Map.unmodifiable(values));
  }

  Future<void> saveExpertConfig(Map<String, Object?> updates) async {
    if (updates.isEmpty) return;

    final settingsUpdates = <String, Object?>{};
    final profileUpdates = <String, Object?>{};
    final unmappedKeys = <String>[];

    for (final entry in updates.entries) {
      final key = entry.key;
      if (_expertSettingsKeys.contains(key)) {
        final modernEntry = _modernExpertSettingsEntry(key, entry.value);
        settingsUpdates[modernEntry.key] = modernEntry.value;
      } else if (_expertProfileConfigKeys.contains(key)) {
        profileUpdates[key] = entry.value;
      } else {
        unmappedKeys.add(key);
      }
    }

    if (unmappedKeys.isNotEmpty) {
      unmappedKeys.sort();
      throw StateError(
        'This build is missing Expert settings mappings for: '
        '${unmappedKeys.join(', ')}',
      );
    }

    if (settingsUpdates.isNotEmpty) {
      await _dio.put('api/settings', data: settingsUpdates);
    }

    if (profileUpdates.isNotEmpty) {
      final base = await _fetchExpertProfileConfig();
      final next = _expertConfigValuesFromServer(base)..addAll(profileUpdates);
      final profileId = _stringValue(base['id']) ?? 'rhythm';
      next['id'] = profileId;
      await _dio.put(
        'api/config',
        queryParameters: {
          'id': profileId,
          'apply': 'true',
        },
        data: next,
      );
    }
  }

  Future<_ExpertOutdoorStatus> fetchOutdoorStatus() async {
    final response = await _dio.get('api/outdoor');
    return _ExpertOutdoorStatus.fromServer(_asMap(response.data));
  }

  Future<void> refreshOutdoor() async {
    await _dio.get('api/outdoor');
  }

  Future<void> learnBaselines() async {
    await _dio.post(
      'api/environment/learn-baselines',
      data: const {'lux_samples': []},
    );
  }

  Future<void> setOutdoorOverride({
    required String condition,
    required int? durationMinutes,
  }) async {
    await _dio.put(
      'api/outdoor/override',
      data: {
        'condition': _modernSkyCondition(condition),
        if (durationMinutes != null) 'expires_in_secs': durationMinutes * 60,
      },
    );
  }

  Future<void> clearOutdoorOverride() async {
    await _dio.put(
      'api/outdoor/override',
      data: const {'clear': true},
    );
  }

  Future<void> postExpertEndpoint(String path) async {
    await _dio.post(path);
  }

  Future<_ExpertMomentsLoad> fetchMoments() async {
    final response = await _dio.get('api/scenes');
    return _ExpertMomentsLoad.fromModernScenes(_asMap(response.data), const {});
  }

  Future<_ExpertMoment> fetchMoment(
    String momentId, {
    required int usageCount,
  }) async {
    final response = await _dio.get('api/scenes');
    final scene = _modernSceneById(_asMap(response.data), momentId);
    if (scene == null) {
      throw StateError(
          'Scene $momentId is missing from the Expert scenes API.');
    }
    return _ExpertMoment.fromModernScene(
      momentId,
      scene,
      usageCount: usageCount,
    );
  }

  Future<List<String>> _momentTargetIds() async {
    var ids = _knownMomentTargetIds();
    if (ids.isNotEmpty) return ids;
    try {
      await fetchZones();
      ids = _knownMomentTargetIds();
    } catch (error) {
      debugPrint('CircadianExpert: moment target refresh failed: $error');
    }
    return ids;
  }

  List<String> _knownMomentTargetIds() {
    final ids = <String>{};
    for (final areaIds in _zoneAreaIds.values) {
      for (final areaId in areaIds) {
        final trimmed = areaId.trim();
        if (trimmed.isNotEmpty) ids.add(trimmed);
      }
    }
    final sorted = ids.toList()..sort();
    return sorted;
  }

  Future<void> createMoment(String name) async {
    final scenesResponse = await _dio.get('api/scenes');
    final existingIds = _modernSceneIds(_asMap(scenesResponse.data));
    final moment = _ExpertMoment(
      id: _uniqueExpertMomentId(name, existingIds),
      name: name,
      icon: 'mdi:palette',
      category: 'utility',
      defaultAction: 'lights_on',
      timerSeconds: 0,
      exceptions: const {},
      usageCount: 0,
    );
    await _dio.post(
      'api/scenes',
      data: _modernSceneForExpertMoment(
        moment,
        targetIds: await _momentTargetIds(),
      ),
    );
  }

  Future<void> runMoment(String momentId) async {
    final response = await _dio.get('api/scenes');
    final scene = _modernSceneById(_asMap(response.data), momentId);
    if (scene == null) {
      throw StateError(
          'Scene $momentId is missing from the Expert scenes API.');
    }
    final moment = _ExpertMoment.fromModernScene(
      momentId,
      scene,
      usageCount: 0,
    );
    final targetIds = await _momentTargetIds();
    if (targetIds.isEmpty) {
      throw StateError('No Expert rooms are available for this moment.');
    }
    final hasSceneLight = _asMapOrNull(scene['light']) != null;
    for (final targetId in targetIds) {
      final exception = moment.exceptions[targetId];
      final action = exception?.action ?? moment.defaultAction;
      if (action == 'leave_alone') continue;
      final autoOffSeconds = exception?.timerSeconds ?? moment.timerSeconds;
      if (action == moment.defaultAction && hasSceneLight) {
        await _dio.post(
          'api/scenes/${Uri.encodeComponent(momentId)}/apply',
          data: {
            'target_id': targetId,
          },
        );
        if (_momentActionArmsAutoOff(action, autoOffSeconds)) {
          await runAreaAction(
            targetId,
            'set_auto_off',
            {'duration_minutes': (autoOffSeconds / 60).round()},
          );
        }
        continue;
      }

      final areaAction = _areaActionForExpertMomentAction(action);
      if (areaAction == null) {
        throw StateError(
          'Moment action ${_momentActionLabel(action)} needs a saved scene layer.',
        );
      }
      await runAreaAction(targetId, areaAction);
      if (_momentActionArmsAutoOff(action, autoOffSeconds)) {
        await runAreaAction(
          targetId,
          'set_auto_off',
          {'duration_minutes': (autoOffSeconds / 60).round()},
        );
      }
    }
  }

  Future<void> deleteMoment(String momentId) async {
    await _dio.delete('api/scenes/${Uri.encodeComponent(momentId)}');
  }

  Future<void> updateMomentFields(
    String momentId,
    Map<String, Object?> fields,
  ) async {
    if (fields.isEmpty) return;
    final response = await _dio.get('api/scenes');
    final scene = _modernSceneById(_asMap(response.data), momentId);
    if (scene == null) {
      throw StateError(
          'Scene $momentId is missing from the Expert scenes API.');
    }
    var next = _ExpertMoment.fromModernScene(
      momentId,
      scene,
      usageCount: 0,
    );
    final name = _stringValue(fields['name']);
    if (name != null) next = next.copyWith(name: name);
    final icon = _stringValue(fields['icon']);
    if (icon != null) next = next.copyWith(icon: icon);
    final category = _stringValue(fields['category']);
    if (category != null) next = next.copyWith(category: category);
    final defaultAction = _stringValue(fields['default_action']);
    if (defaultAction != null) {
      next = next.copyWith(defaultAction: defaultAction);
    }
    if (fields.containsKey('timer')) {
      next = next.copyWith(
        timerSeconds: _intValue(fields['timer'], fallback: next.timerSeconds),
      );
    }
    if (fields.containsKey('exceptions')) {
      next = next.copyWith(
        exceptions: _momentExceptionsFromServer(fields['exceptions']),
      );
    }
    await _dio.put(
      'api/scenes/${Uri.encodeComponent(momentId)}',
      data: _modernSceneForExpertMoment(
        next,
        targetIds: await _momentTargetIds(),
        base: scene,
        rewriteLight: fields.containsKey('default_action'),
      ),
    );
  }

  Future<_ExpertSwitchesLoad> fetchSwitches() async {
    final responses = await Future.wait([
      _dio.get('api/input-bindings'),
      _dio.get('$_removed-projectRuntimeApi/input-actions'),
      _dio.get('$_removed-projectRuntimeApi/switchmap'),
      _dio.get('api/scenes'),
      _dio.get('api/config'),
      _dio.get('api/topology/nodes'),
      _dio.get('$_removed-projectRuntimeApi/scope'),
    ]);
    final configRaw = Map<dynamic, dynamic>.from(_asMap(responses[4].data));
    try {
      final settingsResponse = await _dio.get('api/settings');
      configRaw.addAll(
        _expertSettingsValuesFromServer(_asMap(settingsResponse.data)),
      );
    } on DioException catch (error) {
      if (error.response?.statusCode != 404) rethrow;
    }
    return _ExpertSwitchesLoad.fromModern(
      bindingsRaw: _asMap(responses[0].data),
      actionsRaw: _asMap(responses[1].data),
      switchmapRaw: _asMap(responses[2].data),
      scenesRaw: _asMap(responses[3].data),
      configRaw: configRaw,
      topologyNodes: _topologyNodesFromPayload(responses[5].data),
      scopeRaw: _asMap(responses[6].data),
    );
  }

  Future<_ExpertAreaControlsLoad> fetchAreaControls(String areaId) async {
    final responses = await Future.wait([
      _dio.get('api/input-bindings'),
      _dio.get('api/config'),
      _dio.get('api/topology/nodes'),
    ]);
    final topologyNodes = _topologyNodesFromPayload(responses[2].data);
    var sectionIds = <String>{};
    Map<dynamic, dynamic> scopeRaw = const {};
    try {
      final scope =
          await _dio.get('api/light-runtimes/removed-circadian/scope');
      scopeRaw = _asMap(scope.data);
      sectionIds = _sectionIdsForAreaFromExpertScope(scopeRaw, areaId);
    } on DioException catch (error) {
      if (error.response?.statusCode != 404) rethrow;
    }
    if (sectionIds.isEmpty) {
      sectionIds = _sectionIdsForAreaFromTopology(topologyNodes, areaId);
    }
    final controls = _expertControlsFromModernBindings(
      _asMap(responses[0].data),
      topologyNodes,
      scopeRaw: scopeRaw,
    )
        .where(
          (control) => _controlTargetsArea(
            control,
            areaId,
            sectionIds: sectionIds,
          ),
        )
        .toList();
    controls.sort(
      (a, b) => _compareAreaControls(
        areaId,
        a,
        b,
        sectionIds: sectionIds,
      ),
    );
    return _ExpertAreaControlsLoad(
      controls: controls,
      defaultPauseMinutes: _intValue(
        _asMap(responses[1].data)['default_pause_duration_minutes'],
        fallback: 240,
      ),
    );
  }

  Future<_ExpertControlsRefresh> fetchControlsRefresh() async {
    final response = await _dio.get('api/controls/refresh');
    return _ExpertControlsRefresh.fromServer(_asMap(response.data));
  }

  Future<List<_ExpertControlSourceDevice>> searchControlSourceDevices(
    String query,
  ) async {
    final trimmed = query.trim();
    final response = await _dio.get('api/topology/nodes');
    final devices = _controlSourceDevicesFromTopology(
      _topologyNodesFromPayload(response.data),
      query: trimmed,
    );
    devices.sort((a, b) => a.name.toLowerCase().compareTo(
          b.name.toLowerCase(),
        ));
    return devices;
  }

  Future<String> addControlSource({
    required _ExpertControlSourceDevice device,
    required List<String> triggerEntities,
  }) async {
    final selectedSources = triggerEntities.isEmpty
        ? [device.deviceId]
        : _uniqueOrdered(triggerEntities);
    final existingBindings =
        _asMap((await _dio.get('api/input-bindings')).data);
    String? firstBindingId;

    for (final sourceId in selectedSources) {
      final sensor = device.binarySensors
          .where((candidate) => candidate.entityId == sourceId)
          .firstOrNull;
      final triggerKind = _inputTriggerKindForControlSource(device, sensor);
      final existing = _existingInputBindingForSource(
        existingBindings,
        sourceId: sourceId,
        triggerKind: triggerKind,
      );
      if (existing != null) {
        firstBindingId ??= existing;
        continue;
      }

      final bindingId = _controlSourceBindingId(sourceId, triggerKind);
      await _dio.post(
        'api/input-bindings',
        data: {
          'id': bindingId,
          'name': sensor?.label ?? device.name,
          'source_type': _modernInputBindingTypeForTrigger(triggerKind),
          'source_node_id': sourceId,
          'trigger': {
            'kind': triggerKind,
          },
          'action': _catalogInputBindingActionPayload(
            'circadian_on',
            const [],
          ),
          'targets': const <String>[],
          'target_lists': const <Object?>[],
          'enabled': false,
        },
      );
      firstBindingId ??= bindingId;
    }

    if (firstBindingId == null) {
      throw StateError('No control source was selected');
    }
    return firstBindingId;
  }

  Future<void> reportControlDevice(String deviceId) async {
    await _dio.post(
      'api/devices/report',
      data: {'device_id': deviceId},
    );
  }

  Future<void> setControlPause(
    String controlId, {
    required bool inactive,
    required String? inactiveUntil,
  }) async {
    await _dio.put(
      'api/controls/${Uri.encodeComponent(controlId)}/pause',
      data: inactive
          ? {
              if (_secondsUntil(inactiveUntil) case final seconds?)
                'expires_in_secs': seconds,
            }
          : const {'clear': true},
    );
  }

  Future<void> configureControl(_ExpertControl control) async {
    final binding = await _inputBindingForEdit(control.id, control.rawBinding);
    await _dio.put(
      'api/input-bindings/${Uri.encodeComponent(control.id)}',
      data: _modernInputBindingPayloadFromControl(control, binding),
    );
  }

  Future<void> resetControlConfig(String controlId) async {
    final binding = await _inputBindingForEdit(controlId, null);
    await _dio.put(
      'api/input-bindings/${Uri.encodeComponent(controlId)}',
      data: _modernInputBindingResetPayload(binding),
    );
  }

  Future<void> updateSwitch(_ExpertSwitch control) async {
    final binding = await _inputBindingForEdit(control.id, control.rawBinding);
    await _dio.put(
      'api/input-bindings/${Uri.encodeComponent(control.id)}',
      data: _modernInputBindingPayloadFromSwitch(control, binding),
    );
  }

  Future<void> deleteSwitch(String switchId) async {
    await _dio.delete('api/input-bindings/${Uri.encodeComponent(switchId)}');
  }

  Future<_ExpertSwitchmapLoad> fetchSwitchmap() async {
    final responses = await Future.wait([
      _dio.get('$_removed-projectRuntimeApi/input-actions'),
      _dio.get('$_removed-projectRuntimeApi/switchmap'),
    ]);
    return _ExpertSwitchmapLoad.fromModern(
      actionsRaw: _asMap(responses[0].data),
      switchmapRaw: _asMap(responses[1].data),
    );
  }

  Future<Map<dynamic, dynamic>> _inputBindingForEdit(
    String bindingId,
    Map<dynamic, dynamic>? cached,
  ) async {
    if (cached != null) return cached;
    final response = await _dio.get('api/input-bindings');
    final bindings = _asMap(response.data)['bindings'];
    if (bindings is Iterable) {
      for (final item in bindings) {
        final binding = _asMapOrNull(item);
        if (_stringValue(binding?['id']) == bindingId) return binding!;
      }
    }
    throw StateError(
        'Input binding $bindingId is missing from the Expert controls API.');
  }

  Future<void> saveSwitchmapCustomMappings(
    Map<String, Map<String, Object?>> mappings,
  ) async {
    await _dio.put(
      'api/light-runtimes/removed-circadian/switchmap',
      data: {'custom_mappings': mappings},
    );
  }

  Future<void> saveAreaSettings(
    String areaId,
    Map<String, Object?> updates,
  ) async {
    if (updates.isEmpty) return;
    await _dio.put(
      'api/light-runtimes/removed-circadian/areas/${Uri.encodeComponent(areaId)}/settings',
      data: updates,
    );
  }

  Future<void> saveAreaTune(
    String areaId, {
    double? brightnessFactor,
    double? naturalLightExposure,
  }) async {
    await _dio.put(
      _expertAreaLayerPath(areaId),
      data: {
        'fields': {
          if (brightnessFactor != null) 'balance': brightnessFactor,
          if (naturalLightExposure != null)
            'local_sun_exposure': naturalLightExposure,
        },
      },
    );
  }

  Future<void> saveAreaFeedbackTarget(
    String areaId,
    String? feedbackTarget,
  ) async {
    await _dio.put(
      'api/light-runtimes/removed-circadian/areas/${Uri.encodeComponent(areaId)}/feedback-target',
      data: {'feedback_target': feedbackTarget},
    );
  }

  Future<void> saveLightFilter(
    String areaId,
    String entityId,
    String purpose,
  ) async {
    await _dio.put(
      _expertAreaLightFilterPath(areaId),
      data: {
        'device_id': entityId,
        'preset_id': purpose == 'Standard' ? null : purpose,
      },
    );
  }

  Future<void> saveLightFiltersBulk(
    String areaId,
    Map<String, String> filters,
  ) async {
    if (filters.isEmpty) return;
    for (final entry in filters.entries) {
      await saveLightFilter(areaId, entry.key, entry.value);
    }
  }

  Future<void> flashLight(String entityId) async {
    await _dio.post(
      'api/devices/canonical/${Uri.encodeComponent(entityId)}/flash',
    );
  }

  Future<void> assignLightToSection({
    required String areaId,
    required String entityId,
    required String? sectionId,
  }) async {
    await _dio.put(
      'api/light-runtimes/removed-circadian/areas/${Uri.encodeComponent(areaId)}/light-section',
      data: {
        'device_id': entityId,
        'section_id': sectionId,
      },
    );
  }

  Future<void> createSection(String areaId, String name) async {
    await _dio.post(
      'api/light-runtimes/removed-circadian/areas/${Uri.encodeComponent(areaId)}/sections',
      data: {'name': name},
    );
  }

  Future<void> renameSection(String sectionId, String name) async {
    await _dio.put(
      'api/light-runtimes/removed-circadian/sections/${Uri.encodeComponent(sectionId)}',
      data: {'name': name},
    );
  }

  Future<void> deleteSection(String sectionId) async {
    await _dio.delete(
        'api/light-runtimes/removed-circadian/sections/${Uri.encodeComponent(sectionId)}');
  }

  Future<void> saveScheduleParticipation({
    required String areaId,
    required String participantId,
    required bool isMain,
    bool? participatesInAutoOn,
    bool? participatesInAutoOff,
  }) async {
    await _dio.put(
      isMain
          ? _expertAreaLayerPath(areaId)
          : _expertSectionLayerPath(participantId),
      data: {
        'fields': {
          if (participatesInAutoOn != null)
            'auto_on_participation': participatesInAutoOn,
          if (participatesInAutoOff != null)
            'auto_off_participation': participatesInAutoOff,
        },
      },
    );
  }

  Future<void> saveSectionTune({
    required String areaId,
    required String sectionId,
    required bool isMain,
    required String dimension,
    required double? value,
  }) async {
    final field = switch (dimension) {
      'balance' => 'balance',
      'sun_dimming' => 'local_sun_exposure',
      _ => null,
    };
    if (field == null) return;
    await _dio.put(
      isMain
          ? _expertAreaLayerPath(areaId)
          : _expertSectionLayerPath(sectionId),
      data: {
        if (value == null)
          'clear_fields': [field]
        else
          'fields': {field: value},
      },
    );
  }

  Future<void> toggleSectionPower({
    required String areaId,
    required String sectionId,
    required bool isMain,
  }) async {
    if (isMain) {
      await runAreaAction(areaId, 'toggle');
      return;
    }
    await runSectionAction(sectionId, 'circadian_toggle');
  }

  Future<void> saveSectionBrightness({
    required String areaId,
    required String sectionId,
    required bool isMain,
    required double areaBrightness,
    required double? value,
  }) async {
    if (!isMain) {
      if (value == null) {
        await runSectionAction(sectionId, 'reset_brightness_override');
        return;
      }
      final trim = (value - areaBrightness).clamp(-100, 100).toDouble();
      await _dio.put(
        _expertSectionLayerPath(sectionId),
        data: {
          'fields': {
            'brightness_trim': {'value': trim},
          },
        },
      );
      return;
    }
    final brightness = value?.round().clamp(0, 100).toInt();
    if (brightness == null) {
      await runAreaAction(areaId, 'reset_brightness_override');
      return;
    }
    await runAreaAction(areaId, 'set_brightness', {'brightness': brightness});
  }

  Future<void> setAreaBrightness(String areaId, int brightness) async {
    await runAreaAction(
      areaId,
      'set_brightness',
      {'brightness': brightness.clamp(1, 100).toInt()},
    );
  }

  Future<void> stepSectionBrightness({
    required String areaId,
    required String sectionId,
    required bool isMain,
    required String direction,
  }) async {
    final action = direction == 'down' ? 'bright_down' : 'bright_up';
    if (isMain) {
      await runAreaAction(areaId, action);
      return;
    }
    await runSectionAction(sectionId, action);
  }

  Future<void> setSectionAutoOff({
    required String areaId,
    required String sectionId,
    required bool isMain,
    required int? durationMinutes,
  }) async {
    final action = durationMinutes == null ? 'clear_auto_off' : 'set_auto_off';
    final extra = <String, Object?>{
      if (durationMinutes != null) 'duration_minutes': durationMinutes,
    };
    if (isMain) {
      await runAreaAction(areaId, action, extra);
      return;
    }
    await runSectionAction(sectionId, action, extra);
  }

  Future<void> runAreaAction(
    String areaId,
    String action, [
    Map<String, Object?> extra = const {},
  ]) async {
    await _dio.post(
      _expertAreaActionPath(areaId),
      data: _expertScopeActionPayload(action, extra),
    );
  }

  Future<void> runSectionAction(
    String sectionId,
    String action, [
    Map<String, Object?> extra = const {},
  ]) async {
    await _dio.post(
      _expertSectionActionPath(sectionId),
      data: _expertScopeActionPayload(action, extra),
    );
  }

  Future<void> runZoneAction(
    String zoneName,
    String action, [
    Map<String, Object?> extra = const {},
  ]) async {
    final zoneId = _runtimeZoneIdFor(zoneName);
    await _dio.post(
      'api/light-runtimes/removed-circadian/zones/${Uri.encodeComponent(zoneId)}/action',
      data: _expertScopeActionPayload(action, extra),
    );
  }
}

class _ZoneLoadResult {
  final List<_ExpertZone> visibleZones;
  final List<String> allOrder;

  const _ZoneLoadResult({
    required this.visibleZones,
    required this.allOrder,
  });
}

_ZoneLoadResult _zonesFromExpertScope(
  Map<dynamic, dynamic> raw,
  _RhythmDefinition definition,
) {
  final rawZones = raw['zones'];
  if (rawZones is! Iterable) {
    return const _ZoneLoadResult(visibleZones: [], allOrder: []);
  }

  final zones = <_ExpertZone>[];
  for (final rawZone in rawZones) {
    final zone = _asMapOrNull(rawZone);
    if (zone == null) continue;
    final id = _stringValue(zone['id']) ?? _stringValue(zone['name']);
    final name = _stringValue(zone['name']) ?? id;
    if (id == null || name == null) continue;

    final areas = <_ExpertArea>[];
    final rawAreas = zone['areas'];
    if (rawAreas is Iterable) {
      for (final rawArea in rawAreas) {
        final area = _expertAreaFromScope(rawArea);
        if (area != null) areas.add(area);
      }
    }

    zones.add(
      _ExpertZone(
        id: id,
        name: name,
        isDefault: _boolValue(zone['is_default'], fallback: false),
        areas: List.unmodifiable(areas),
        definition: definition,
        currentState: null,
      ),
    );
  }

  final allOrder = _stringList(raw['all_order']);
  return _ZoneLoadResult(
    visibleZones: List.unmodifiable(zones),
    allOrder: allOrder.isEmpty
        ? zones.map((zone) => zone.id).toList(growable: false)
        : allOrder,
  );
}

_ExpertArea? _expertAreaFromScope(Object? rawArea) {
  final area = _asMapOrNull(rawArea);
  if (area == null) return null;
  final id = _stringValue(area['id']);
  if (id == null) return null;
  final rawSections = area['sections'];
  final sections = <_ExpertSectionOption>[];
  if (rawSections is Iterable) {
    for (final rawSection in rawSections) {
      final section = _asMapOrNull(rawSection);
      final sectionId = _stringValue(section?['id']);
      if (sectionId == null) continue;
      sections.add(
        _ExpertSectionOption(
          id: sectionId,
          name: _stringValue(section?['name']) ?? sectionId,
        ),
      );
    }
  }

  return _ExpertArea(
    id: id,
    name: _stringValue(area['name']) ?? id,
    deviceCount: _intValue(area['device_count'], fallback: 0),
    brightnessOffset: _intValue(area['brightness_offset'], fallback: 0),
    sections: List.unmodifiable(sections),
    lightsOn: _boolValue(_asMapOrNull(area['effective'])?['hard_off'],
            fallback: false)
        ? false
        : null,
    brightness: _scopeBrightnessOverride(area['effective'])?.round(),
    kelvin: null,
    stale: false,
  );
}

_ZoneLoadResult _zonesFromModernRooms(List<RhythmRoom> rooms) {
  final areas = rooms.map(_areaFromModernRoom).toList(growable: false);
  if (areas.isEmpty) {
    return const _ZoneLoadResult(
      visibleZones: [],
      allOrder: [],
    );
  }

  const zoneName = 'Rhythm Topology';
  return _ZoneLoadResult(
    visibleZones: [
      _ExpertZone(
        id: zoneName,
        name: zoneName,
        isDefault: true,
        areas: areas,
        definition: _definitionFromModernRooms(rooms),
        currentState: _aggregateModernZoneState(rooms),
      ),
    ],
    allOrder: const [zoneName],
  );
}

double? _scopeBrightnessOverride(Object? rawFields) {
  final fields = _asMapOrNull(rawFields);
  final override = _asMapOrNull(fields?['brightness_override']);
  return _nullableDouble(override?['value']);
}

double? _scopeBrightnessTrim(Object? rawFields) {
  final fields = _asMapOrNull(rawFields);
  final trim = _asMapOrNull(fields?['brightness_trim']);
  return _nullableDouble(trim?['value']);
}

double? _scopeMidpoint(Object? rawFields) {
  final fields = _asMapOrNull(rawFields);
  final rawMid = fields?['mid'];
  if (rawMid is Map) {
    return _nullableDouble(rawMid['hours'] ?? rawMid['value']);
  }
  return _nullableDouble(rawMid);
}

List<RhythmRoom> _combinedModernRooms({
  required List<RhythmRoom> stateNodes,
  required List<RhythmTopologyNode> topologyNodes,
}) {
  final stateById = <String, RhythmRoom>{
    for (final node in stateNodes) node.id: node,
  };

  if (topologyNodes.isEmpty) {
    final rooms = stateNodes.where((node) => node.kind.isRoom).toList();
    rooms.sort((left, right) => left.name.compareTo(right.name));
    return rooms;
  }

  final roomChildren = <String, List<RhythmTopologyNode>>{};
  for (final node in topologyNodes.where((node) => node.isDevice)) {
    final parentId = node.parentId;
    if (parentId == null || parentId.isEmpty) continue;
    (roomChildren[parentId] ??= []).add(node);
  }

  final rooms = <RhythmRoom>[];
  for (final topologyRoom in topologyNodes.where((node) => node.isRoom)) {
    final state = stateById[topologyRoom.id];
    final devices = (roomChildren[topologyRoom.id] ?? const [])
        .map(RhythmDevice.fromTopologyNode)
        .toList()
      ..sort((left, right) {
        const order = {
          RhythmDeviceType.light: 0,
          RhythmDeviceType.button: 1,
          RhythmDeviceType.motion: 2,
        };
        return (order[left.type] ?? 3).compareTo(order[right.type] ?? 3);
      });

    rooms.add(
      RhythmRoom(
        id: topologyRoom.id,
        name: topologyRoom.name,
        kind: topologyRoom.kind,
        parentId: topologyRoom.parentId,
        placement: topologyRoom.placement,
        groupedLightId: state?.groupedLightId ?? '',
        state: state?.state ?? RoomModeState.active,
        transitioning: state?.transitioning ?? false,
        rhythmEnabled: state?.rhythmEnabled ?? false,
        disabled: state?.disabled ?? false,
        timeOffset: state?.timeOffset ?? 0,
        brightnessOffset: state?.brightnessOffset ?? 0,
        hubTypes: state?.hubTypes ??
            topologyRoom.hubRoomBindings
                .map((binding) => binding.hubKey?['hub_type']?.toString())
                .whereType<String>()
                .toSet()
                .toList(),
        manufacturer: state?.manufacturer,
        model: state?.model,
        deviceIds: devices
            .where((device) => device.type == RhythmDeviceType.light)
            .map((device) => device.id)
            .toList(),
        devices: devices,
        profileSettings: state?.profileSettings,
        observedPower: state?.observedPower,
        lightsOn: state?.lightsOn,
        brightness: state?.brightness,
        kelvin: state?.kelvin,
        moodEnabled: state?.moodEnabled ?? false,
        moodActive: state?.moodActive ?? false,
        standbyEnabled: state?.standbyEnabled ?? false,
        standbyActive: state?.standbyActive ?? false,
        motionActive: state?.motionActive,
        motionOwned: state?.motionOwned,
        remainingSecs: state?.remainingSecs,
        timeoutSecs: state?.timeoutSecs,
        warningActive: state?.warningActive,
      ),
    );
  }

  rooms.sort((left, right) => left.name.compareTo(right.name));
  return rooms;
}

List<RhythmTopologyNode> _topologyNodesFromPayload(Object? payload) {
  final rawNodes =
      payload is Iterable ? payload : _asMapOrNull(payload)?['nodes'];
  if (rawNodes is! Iterable) return const [];
  return rawNodes
      .map(_stringMapOrNull)
      .whereType<Map<String, dynamic>>()
      .map(RhythmTopologyNode.fromJson)
      .where((node) => node.id.isNotEmpty)
      .toList(growable: false);
}

_ExpertArea _areaFromModernRoom(RhythmRoom room) {
  final sections = <_ExpertSectionOption>[];
  final sectionIds = <String>{};
  for (final light in room.lights) {
    if (!sectionIds.add(light.id)) continue;
    sections.add(
      _ExpertSectionOption(
        id: light.id,
        name: light.displayName,
      ),
    );
  }
  for (final id in room.deviceIds) {
    if (!sectionIds.add(id)) continue;
    sections.add(_ExpertSectionOption(id: id, name: id));
  }

  final lightCount = room.lightCount;
  return _ExpertArea(
    id: room.id,
    name: room.name.isEmpty ? room.id : room.name,
    deviceCount: lightCount > 0 ? lightCount : room.deviceCount,
    brightnessOffset: room.brightnessOffset.round(),
    sections: List.unmodifiable(sections),
    lightsOn: room.lightsOn,
    brightness: room.brightness?.clamp(0, 100).toInt(),
    kelvin: room.kelvin?.clamp(1500, 7000).toInt(),
    stale: room.disabled,
  );
}

_ExpertZoneState? _aggregateModernZoneState(List<RhythmRoom> rooms) {
  final brightnessValues =
      rooms.map((room) => room.brightness).whereType<int>();
  final kelvinValues = rooms.map((room) => room.kelvin).whereType<int>();
  if (brightnessValues.isEmpty && kelvinValues.isEmpty) return null;

  final brightness =
      _averageInt(brightnessValues, fallback: 50).clamp(0, 100).toInt();
  return _ExpertZoneState(
    brightness: brightness,
    actualBrightness: brightness,
    kelvin: _averageInt(kelvinValues, fallback: 4000).clamp(1500, 7000).toInt(),
    minBrightness: 1,
    maxBrightness: 100,
    brightnessSensitivity: null,
    frozen: rooms.any((room) => room.transitioning),
    hasRuntimeAdjustment: rooms.any(
      (room) =>
          room.brightnessOffset.abs() > 0.05 || room.timeOffset.abs() > 0.05,
    ),
  );
}

String? _areaActionForExpertMomentAction(String action) {
  return switch (action) {
    'lights_on' || 'on' => 'lights_on',
    'lights_off' || 'off' => 'lights_off',
    'lights_toggle' || 'toggle' => 'lights_toggle',
    'nitelite' => 'set_nitelite',
    'britelite' => 'set_britelite',
    'wake_or_bed' => 'set_wake_or_bed',
    'reset' || 'glo_reset' => 'glo_reset',
    'circadian_on' ||
    'circadian_off' ||
    'bright_up' ||
    'bright_down' ||
    'step_up' ||
    'step_down' =>
      action,
    _ => null,
  };
}

bool _momentActionArmsAutoOff(String action, int autoOffSeconds) {
  if (autoOffSeconds <= 0) return false;
  return action != 'off' && action != 'lights_off';
}

int _averageInt(Iterable<int> values, {required int fallback}) {
  var count = 0;
  var total = 0;
  for (final value in values) {
    count++;
    total += value;
  }
  if (count == 0) return fallback;
  return (total / count).round();
}

_RhythmDefinition _definitionFromModernRooms(List<RhythmRoom> rooms) {
  final active =
      rooms.map((room) => room.profileSettings).whereType<Object>().isNotEmpty;
  return _RhythmDefinition(
    sleepPattern: active ? 'adult' : 'standard',
    wakeHour: 7,
    bedHour: 22.5,
    minBrightness: 18,
    maxBrightness: 92,
    sleepBrightness: 8,
    minKelvin: 2200,
    maxKelvin: 5200,
    daylightDimming: 0.25,
    naturalExposure: 0.35,
    transitionMinutes: 35,
    phaseBalance: 0,
  );
}

_RhythmDefinition _rhythmDefinitionFromExpertProfile(
    Map<dynamic, dynamic> raw) {
  final curve = _asMapOrNull(raw['curve']) ?? const {};
  final schedule = _asMapOrNull(curve['schedule']) ?? const {};
  final wakeHour = _normalizeHour(
    _doubleValue(
      _asMapOrNull(schedule['wake'])?['hour'],
      fallback: 7,
    ),
  );
  final bedHour = _normalizeHour(
    _doubleValue(
      _asMapOrNull(schedule['bed'])?['hour'],
      fallback: 22,
    ),
  );
  final minBrightness =
      _intValue(raw['min_brightness'], fallback: 18).clamp(1, 100).toInt();
  final maxBrightness = math
      .max(
        minBrightness + 1,
        _intValue(raw['max_brightness'], fallback: 92).clamp(1, 100).toInt(),
      )
      .clamp(1, 100)
      .toInt();
  final minKelvin = _intValue(raw['min_color_temp'], fallback: 2200)
      .clamp(1500, 7000)
      .toInt();
  final maxKelvin = math
      .max(
        minKelvin + _expertKelvinStep,
        _intValue(raw['max_color_temp'], fallback: 5200)
            .clamp(1500, 7000)
            .toInt(),
      )
      .clamp(1500, 7000)
      .toInt();
  final bedBrightness = _brightnessForTargetPercent(
    _intValue(curve['bed_brightness'], fallback: 50),
    minBrightness,
    maxBrightness,
  );
  final speed = _averageInt([
    _intValue(curve['wake_speed'], fallback: 8),
    _intValue(curve['bed_speed'], fallback: 6),
  ], fallback: 7);

  return _RhythmDefinition(
    sleepPattern: _stringValue(raw['id']) ?? _expertProfileId,
    wakeHour: wakeHour,
    bedHour: bedHour,
    minBrightness: minBrightness,
    maxBrightness: maxBrightness,
    sleepBrightness: bedBrightness,
    minKelvin: minKelvin,
    maxKelvin: maxKelvin,
    daylightDimming: 0.25,
    naturalExposure: 0.35,
    transitionMinutes: _transitionMinutesFromSigmoidSpeed(speed),
    phaseBalance: _phaseBalanceFromAscend(
      _doubleValue(curve['ascend_start'], fallback: wakeHour - 3.5),
      wakeHour,
    ),
  );
}

Map<String, dynamic> _expertProfileConfigFromDefinition(
  _RhythmDefinition definition, {
  Map<dynamic, dynamic>? base,
}) {
  final minBrightness = definition.minBrightness.clamp(1, 99).toInt();
  final maxBrightness = math
      .max(minBrightness + 1, definition.maxBrightness)
      .clamp(2, 100)
      .toInt();
  final minKelvin = definition.minKelvin.clamp(1500, 6749).toInt();
  final maxKelvin = math
      .max(minKelvin + _expertKelvinStep, definition.maxKelvin)
      .clamp(1750, 7000)
      .toInt();
  final speed = _sigmoidSpeedFromTransitionMinutes(
    definition.transitionMinutes,
  );
  final next = <String, dynamic>{};
  if (base != null) {
    for (final entry in base.entries) {
      final key = _stringValue(entry.key);
      if (key != null) next[key] = entry.value;
    }
  }

  next
    ..['id'] = _expertProfileId
    ..['name'] = _stringValue(next['name']) ?? 'Expert'
    ..['min_brightness'] = minBrightness
    ..['max_brightness'] = maxBrightness
    ..['min_color_temp'] = minKelvin
    ..['max_color_temp'] = maxKelvin
    ..['curve'] = {
      'type': 'sigmoid',
      'schedule': {
        'wake': {'hour': _roundHundredth(definition.wakeHour)},
        'bed': {'hour': _roundHundredth(definition.bedHour)},
        'alternate_days': <dynamic>[],
      },
      'ascend_start': _roundHundredth(
        _normalizeHour(definition.wakeHour - 3.5 + definition.phaseBalance),
      ),
      'descend_start': _roundHundredth(
        _normalizeHour(definition.bedHour - 4.0),
      ),
      'wake_speed': speed,
      'bed_speed': speed,
      'wake_brightness': 50,
      'bed_brightness': _targetPercentForBrightness(
        definition.sleepBrightness,
        minBrightness,
        maxBrightness,
      ),
    };
  return next;
}

int _targetPercentForBrightness(int brightness, int min, int max) {
  final span = math.max(1, max - min);
  final normalized = ((brightness - min) / span * 100).round();
  return normalized.clamp(0, 100).toInt();
}

int _brightnessForTargetPercent(int percent, int min, int max) {
  final clamped = percent.clamp(0, 100).toInt();
  return (min + (max - min) * clamped / 100).round().clamp(1, 100).toInt();
}

int _sigmoidSpeedFromTransitionMinutes(double minutes) {
  final normalized = ((minutes.clamp(5, 90) - 5) / 85).clamp(0.0, 1.0);
  return (12 - normalized * 11).round().clamp(1, 12).toInt();
}

double _transitionMinutesFromSigmoidSpeed(int speed) {
  final normalized = ((12 - speed.clamp(1, 12)) / 11).clamp(0.0, 1.0);
  return _roundHundredth(5 + normalized * 85);
}

@visibleForTesting
Map<String, Object?> expertProfileConfigFromDefinitionForTest({
  required double wakeHour,
  required double bedHour,
  required int minBrightness,
  required int maxBrightness,
  required int sleepBrightness,
  required int minKelvin,
  required int maxKelvin,
  required double transitionMinutes,
  required double phaseBalance,
  Map<dynamic, dynamic>? base,
}) =>
    _expertProfileConfigFromDefinition(
      _RhythmDefinition(
        sleepPattern: _expertProfileId,
        wakeHour: wakeHour,
        bedHour: bedHour,
        minBrightness: minBrightness,
        maxBrightness: maxBrightness,
        sleepBrightness: sleepBrightness,
        minKelvin: minKelvin,
        maxKelvin: maxKelvin,
        daylightDimming: 0,
        naturalExposure: 0,
        transitionMinutes: transitionMinutes,
        phaseBalance: phaseBalance,
      ),
      base: base,
    );

@visibleForTesting
Map<String, Object?> expertDefinitionValuesFromProfileForTest(
  Map<dynamic, dynamic> raw,
) {
  final definition = _rhythmDefinitionFromExpertProfile(raw);
  return {
    'sleep_pattern': definition.sleepPattern,
    'wake_hour': definition.wakeHour,
    'bed_hour': definition.bedHour,
    'min_brightness': definition.minBrightness,
    'max_brightness': definition.maxBrightness,
    'sleep_brightness': definition.sleepBrightness,
    'min_kelvin': definition.minKelvin,
    'max_kelvin': definition.maxKelvin,
    'transition_minutes': definition.transitionMinutes,
    'phase_balance': definition.phaseBalance,
  };
}

@visibleForTesting
Map<String, Object?> modernSceneForExpertMomentForTest({
  required String id,
  required String name,
  required String defaultAction,
  required Iterable<String> targetIds,
  String icon = 'mdi:palette',
  String category = 'utility',
  int timerSeconds = 0,
  Map<String, Map<String, Object?>> exceptions = const {},
  Map<dynamic, dynamic>? base,
  bool rewriteLight = true,
}) {
  return _modernSceneForExpertMoment(
    _ExpertMoment(
      id: id,
      name: name,
      icon: icon,
      category: category,
      defaultAction: defaultAction,
      timerSeconds: timerSeconds,
      exceptions: {
        for (final entry in exceptions.entries)
          entry.key: _ExpertMomentException(
            action: _stringValue(entry.value['action']) ?? 'leave_alone',
            timerSeconds: _intValue(entry.value['timer'], fallback: 0),
          ),
      },
      usageCount: 0,
    ),
    targetIds: targetIds,
    base: base,
    rewriteLight: rewriteLight,
  );
}

@visibleForTesting
Map<String, Object?> expertMomentValuesFromModernSceneForTest(
  Map<dynamic, dynamic> raw,
) {
  final moment = _ExpertMoment.fromModernScene(
    _stringValue(raw['id']) ?? 'moment',
    raw,
    usageCount: 0,
  );
  return {
    'id': moment.id,
    'name': moment.name,
    'icon': moment.icon,
    'category': moment.category,
    'default_action': moment.defaultAction,
    'timer': moment.timerSeconds,
    'exceptions': _momentExceptionsToServer(moment.exceptions),
  };
}

@visibleForTesting
List<Map<String, Object?>> expertControlSummariesFromModernBindingsForTest({
  required Map<dynamic, dynamic> bindingsRaw,
  Object? topologyNodesRaw = const [],
  Map<dynamic, dynamic> scopeRaw = const {},
}) {
  return _expertControlsFromModernBindings(
    bindingsRaw,
    _topologyNodesFromPayload(topologyNodesRaw),
    scopeRaw: scopeRaw,
  )
      .map(
        (control) => {
          'id': control.id,
          'name': control.name,
          'category': control.category,
          'status': control.status,
          'supported': control.supported,
          'inactive': control.inactive,
          'type': control.type,
          'magic_buttons': control.magicButtons,
          'scopes': [
            for (final scope in control.scopes)
              {
                'area_ids': scope.areaIds,
                'section_ids': scope.sectionIds,
                'feedback_area': scope.feedbackArea,
                'mode': scope.mode,
              },
          ],
        },
      )
      .toList(growable: false);
}

@visibleForTesting
Map<String, Object?> expertInputBindingActionPayloadForTest({
  required String actionId,
  Iterable<String> areaIds = const [],
  Iterable<String> sectionIds = const [],
}) {
  final areas = areaIds.toList(growable: false);
  final sections = sectionIds.toList(growable: false);
  return _catalogInputBindingActionPayload(
    actionId,
    [
      if (areas.isNotEmpty || sections.isNotEmpty)
        _ExpertSwitchScope(
          areaIds: areas,
          sectionIds: sections,
          feedbackArea: null,
        ),
    ],
  );
}

String _expertAreaActionPath(String areaId) {
  return 'api/light-runtimes/removed-circadian/areas/${Uri.encodeComponent(areaId)}/action';
}

String _expertAreaNowPath(String areaId) {
  return 'api/light-runtimes/removed-circadian/areas/${Uri.encodeComponent(areaId)}/now';
}

String _expertAreaLayerPath(String areaId) {
  return 'api/light-runtimes/removed-circadian/areas/${Uri.encodeComponent(areaId)}/layer';
}

String _expertAreaLightFilterPath(String areaId) {
  return 'api/light-runtimes/removed-circadian/areas/${Uri.encodeComponent(areaId)}/light-filter';
}

String _expertSectionActionPath(String sectionId) {
  return 'api/light-runtimes/removed-circadian/sections/${Uri.encodeComponent(sectionId)}/action';
}

String _expertSectionLayerPath(String sectionId) {
  return 'api/light-runtimes/removed-circadian/sections/${Uri.encodeComponent(sectionId)}/layer';
}

Map<String, Object?> _expertScopeActionPayload(
  String action,
  Map<String, Object?> extra,
) {
  return {
    'action': action,
    ...extra,
  };
}

@visibleForTesting
Map<String, Object?> expertAreaActionRequestForTest({
  required String areaId,
  required String action,
  Map<String, Object?> extra = const {},
}) {
  return {
    'path': _expertAreaActionPath(areaId),
    'data': _expertScopeActionPayload(action, extra),
  };
}

@visibleForTesting
String expertAreaNowPathForTest(String areaId) {
  return _expertAreaNowPath(areaId);
}

@visibleForTesting
Map<String, Object?> expertAreaLayerRequestForTest({
  required String areaId,
  required Map<String, Object?> data,
}) {
  return {
    'path': _expertAreaLayerPath(areaId),
    'data': data,
  };
}

@visibleForTesting
Map<String, Object?> expertAreaLightFilterRequestForTest({
  required String areaId,
  required String deviceId,
  required String purpose,
}) {
  return {
    'path': _expertAreaLightFilterPath(areaId),
    'data': {
      'device_id': deviceId,
      'preset_id': purpose == 'Standard' ? null : purpose,
    },
  };
}

@visibleForTesting
String? expertAreaActionForMomentActionForTest(String action) {
  return _areaActionForExpertMomentAction(action);
}

@visibleForTesting
Map<String, Object?> expertSectionActionRequestForTest({
  required String sectionId,
  required String action,
  Map<String, Object?> extra = const {},
}) {
  return {
    'path': _expertSectionActionPath(sectionId),
    'data': _expertScopeActionPayload(action, extra),
  };
}

@visibleForTesting
Map<String, Object?> expertSectionLayerRequestForTest({
  required String sectionId,
  required Map<String, Object?> data,
}) {
  return {
    'path': _expertSectionLayerPath(sectionId),
    'data': data,
  };
}

@visibleForTesting
Map<String, Object?> expertSwitchmapLoadSummaryForTest({
  required Map<dynamic, dynamic> actionsRaw,
  Map<dynamic, dynamic> switchmapRaw = const {},
}) {
  final load = _ExpertSwitchmapLoad.fromModern(
    actionsRaw: actionsRaw,
    switchmapRaw: switchmapRaw,
  );
  return {
    'type_ids': load.types.keys.toList(growable: false),
    if (load.types['button'] case final buttonType?)
      'button_effective_mapping': buttonType.effectiveMapping,
    if (load.customMappings['button'] case final buttonCustom?)
      'button_custom_mapping': buttonCustom,
    'when_off_options': [
      for (final option in load.whenOffOptions) option.id,
    ],
    'when_off_labels': {
      for (final option in load.whenOffOptions)
        if (option.id != null) option.id!: option.label,
    },
  };
}

Set<String> _sectionIdsForAreaFromTopology(
  List<RhythmTopologyNode> topologyNodes,
  String areaId,
) {
  return topologyNodes
      .where(
        (node) =>
            node.parentId == areaId &&
            RhythmDeviceType.fromNodeKind(node.kind) == RhythmDeviceType.light,
      )
      .map((node) => node.id)
      .toSet();
}

Set<String> _sectionIdsForAreaFromExpertScope(
  Map<dynamic, dynamic> raw,
  String areaId,
) {
  final area = _scopeAreaById(raw, areaId);
  final rawSections = area?['sections'];
  if (rawSections is! Iterable) return const {};
  return {
    for (final rawSection in rawSections)
      if (_stringValue(_asMapOrNull(rawSection)?['id']) case final id?) id,
  };
}

Map<dynamic, dynamic>? _modernSceneById(
  Map<dynamic, dynamic> raw,
  String sceneId,
) {
  final rawScenes = raw['scenes'];
  if (rawScenes is! Iterable) return null;
  for (final scene in rawScenes) {
    final body = _asMapOrNull(scene);
    if (_stringValue(body?['id']) == sceneId) return body;
  }
  return null;
}

Set<String> _modernSceneIds(Map<dynamic, dynamic> raw) {
  final rawScenes = raw['scenes'];
  if (rawScenes is! Iterable) return const {};
  return {
    for (final scene in rawScenes)
      if (_stringValue(_asMapOrNull(scene)?['id']) case final id?) id,
  };
}

String _uniqueExpertMomentId(String name, Set<String> existingIds) {
  final base = _expertMomentIdForName(name);
  if (!existingIds.contains(base)) return base;
  for (var i = 2; i < 1000; i++) {
    final candidate = '$base-$i';
    if (!existingIds.contains(candidate)) return candidate;
  }
  return '$base-${DateTime.now().millisecondsSinceEpoch}';
}

String _expertMomentIdForName(String name) {
  final normalized = name
      .trim()
      .toLowerCase()
      .replaceAll(RegExp(r'[^a-z0-9]+'), '-')
      .replaceAll(RegExp(r'^-+|-+$'), '');
  return normalized.isEmpty ? 'moment' : normalized;
}

bool _modernSceneHasOffOutput(Map<dynamic, dynamic>? light) {
  if (light == null) return false;
  bool isOffOutput(Object? raw) {
    return _stringValue(_asMapOrNull(raw)?['power']) == 'off';
  }

  if (isOffOutput(light['default_output'])) return true;
  final palette = light['palette'];
  if (palette is Iterable && palette.any(isOffOutput)) return true;
  final entries = light['entries'];
  if (entries is Iterable) {
    for (final entry in entries) {
      final body = _asMapOrNull(entry);
      if (body == null) continue;
      if (isOffOutput(body['output'])) return true;
      final value = _asMapOrNull(body['value']);
      if (isOffOutput(value?['output'])) return true;
    }
  }
  return false;
}

String? _modernScenePresetAction(Map<dynamic, dynamic>? light) {
  if (light == null) return null;
  String? presetFromValue(Object? raw) {
    final value = _asMapOrNull(raw);
    if (_stringValue(value?['kind']) != 'preset') return null;
    return _stringValue(value?['preset']);
  }

  final entries = light['entries'];
  if (entries is Iterable) {
    for (final entry in entries) {
      final preset = presetFromValue(_asMapOrNull(entry)?['value']);
      if (preset != null) return preset;
    }
  }
  return null;
}

Map<String, _ExpertMomentException> _momentExceptionsFromServer(Object? raw) {
  final source = _asMapOrNull(raw);
  if (source == null || source.isEmpty) return const {};
  return Map.unmodifiable({
    for (final entry in source.entries)
      if (_asMapOrNull(entry.value) case final body?)
        entry.key.toString(): _ExpertMomentException(
          action: _stringValue(body['action']) ?? 'leave_alone',
          timerSeconds: _intValue(body['timer'], fallback: 0),
        ),
  });
}

Map<String, Object?> _modernSceneForExpertMoment(
  _ExpertMoment moment, {
  required Iterable<String> targetIds,
  Map<dynamic, dynamic>? base,
  bool rewriteLight = true,
}) {
  final targets = targetIds
      .map((id) => id.trim())
      .where((id) => id.isNotEmpty)
      .toSet()
      .toList()
    ..sort();
  final extensions = <String, Object?>{
    for (final entry in (_asMapOrNull(base?['extensions']) ?? const {}).entries)
      entry.key.toString(): entry.value,
  };
  extensions[_expertMomentExtensionKey] = {
    'icon': moment.icon,
    'category': moment.category,
    'default_action': moment.defaultAction,
    'timer': moment.timerSeconds,
    'exceptions': _momentExceptionsToServer(moment.exceptions),
  };

  return {
    ...{
      for (final entry in (base ?? const {}).entries)
        entry.key.toString(): entry.value,
    },
    'id': moment.id,
    'name': moment.name,
    'source': base?['source'] ?? const {'kind': 'user'},
    if (rewriteLight)
      'light': _modernMomentLightLayer(moment.defaultAction, targets)
    else
      'light': base?['light'],
    'extensions': extensions,
  };
}

Map<String, Object?>? _modernMomentLightLayer(
  String action,
  List<String> targetIds,
) {
  final value = _modernSceneValueForMomentAction(action);
  if (targetIds.isNotEmpty && value != null) {
    return {
      'default_transition_ms': 400,
      'entries': [
        for (final targetId in targetIds)
          {
            'target': {
              'kind': 'node',
              'node_id': targetId,
            },
            'value': value,
          },
      ],
    };
  }

  final output = _modernDefaultOutputForMomentAction(action);
  if (output == null) return null;
  return {
    'default_transition_ms': 400,
    'default_output': output,
  };
}

Map<String, Object?>? _modernSceneValueForMomentAction(String action) {
  if (action == 'leave_alone') return const {'kind': 'leave_alone'};
  final preset = _modernLightPresetForMomentAction(action);
  if (preset == null) return null;
  return {
    'kind': 'preset',
    'preset': preset,
  };
}

String? _modernLightPresetForMomentAction(String action) {
  return switch (action) {
    'lights_on' || 'on' => 'lights_on',
    'off' || 'lights_off' => 'off',
    'nitelite' => 'nitelite',
    'britelite' => 'britelite',
    'wake_or_bed' => 'wake_or_bed',
    'circadian_off' => 'circadian_off',
    'reset' => 'reset',
    _ => null,
  };
}

Map<String, Object?>? _modernDefaultOutputForMomentAction(String action) {
  return switch (action) {
    'lights_on' || 'on' => const {
        'power': 'on',
        'brightness': 100,
        'color': {
          'kind': 'kelvin',
          'kelvin': 4000,
        },
      },
    'off' || 'lights_off' => const {
        'power': 'off',
        'brightness': 100,
      },
    _ => null,
  };
}

List<_ExpertControl> _expertControlsFromModernBindings(
  Map<dynamic, dynamic> raw,
  List<RhythmTopologyNode> topologyNodes, {
  Map<dynamic, dynamic> scopeRaw = const {},
}) {
  final nodeById = {for (final node in topologyNodes) node.id: node};
  final scopeIndex = _ExpertScopeBindingIndex.fromScope(scopeRaw);
  final controls = <_ExpertControl>[];
  final bindings = raw['bindings'];
  if (bindings is Iterable) {
    for (final item in bindings) {
      final binding = _asMapOrNull(item);
      if (binding == null) continue;
      final id = _stringValue(binding['id']);
      if (id == null || id.isEmpty) continue;
      final sourceId = _stringValue(binding['source_node_id']);
      final source = sourceId == null ? null : nodeById[sourceId];
      final trigger = _asMapOrNull(binding['trigger']) ?? const {};
      final triggerKind = _stringValue(trigger['kind']);
      final category = _modernControlCategory(source, triggerKind);
      final actionId = _inputBindingActionId(binding['action']);
      final explicitTargetIds = _modernBindingTargetIds(binding);
      final area = _modernBindingArea(
        binding,
        nodeById,
        source,
        scopeIndex: scopeIndex,
      );
      final scopes = _modernBindingScopes(
        binding,
        nodeById,
        source,
        scopeIndex: scopeIndex,
      );
      final buttonAction = _stringValue(trigger['button_action']) ?? 'any';
      final needsIntegration =
          _inputBindingActionIsUnsupportedImported(binding['action']);
      final inactive = !_boolValue(binding['enabled'], fallback: true);
      final notConfigured = inactive && explicitTargetIds.isEmpty;
      final type = _modernInputBindingType(
        binding,
        source: source,
        triggerKind: triggerKind,
      );
      controls.add(
        _ExpertControl(
          id: id,
          name: _stringValue(binding['name']) ?? source?.name ?? sourceId ?? id,
          category: category,
          status: needsIntegration
              ? 'needs_integration'
              : notConfigured
                  ? 'not_configured'
                  : inactive
                      ? 'inactive'
                      : 'active',
          type: type,
          typeName: _modernInputBindingTypeLabel(type, category),
          deviceId: sourceId,
          areaId: area?.id,
          areaName: area?.name,
          manufacturer: source?.manufacturer,
          model: source?.model,
          integration: 'modern_topology',
          supported: !needsIntegration,
          inactive: inactive,
          inactiveUntil: null,
          stale: false,
          batteryLevel: null,
          illuminance: null,
          lastAction: null,
          scopes: scopes,
          binarySensors: source == null
              ? const []
              : [
                  _ExpertTriggerEntity(
                    entityId: source.id,
                    name: source.name,
                  ),
                ],
          magicButtons: {
            if (actionId != null) buttonAction: actionId,
          },
          rawBinding: Map<dynamic, dynamic>.unmodifiable(binding),
        ),
      );
    }
  }
  controls.sort((a, b) => a.name.toLowerCase().compareTo(b.name.toLowerCase()));
  return List.unmodifiable(controls);
}

Map<String, _ExpertSwitchType> _expertSwitchTypesFromModernActions(
    Map<dynamic, dynamic> raw,
    {Map<dynamic, dynamic> switchmapRaw = const {}}) {
  final actionTypes = <String>[];
  final rawActions = raw['actions'];
  if (rawActions is Iterable) {
    for (final action in rawActions) {
      final body = _asMapOrNull(action);
      final id = _stringValue(
        body?['python_action_id'] ??
            body?['migrated_id'] ??
            body?['rust_action'],
      );
      if (id != null && !actionTypes.contains(id)) actionTypes.add(id);
    }
  }
  if (actionTypes.isEmpty) actionTypes.add('mode_cycle');
  final mappings = _asMapOrNull(switchmapRaw['mappings']);
  if (mappings != null && mappings.isNotEmpty) {
    final result = <String, _ExpertSwitchType>{};
    for (final entry in mappings.entries) {
      final id = entry.key?.toString();
      final body = _asMapOrNull(entry.value);
      if (id == null || id.isEmpty || body == null) continue;
      result[id] = _ExpertSwitchType(
        id: id,
        name: _stringValue(body['name']) ?? _titleCase(id),
        buttons: _orderedStringList(body['buttons']),
        actionTypes: _orderedStringList(body['action_types']),
        defaultMapping: _objectMap(body['effective_mapping']),
      );
    }
    if (result.isNotEmpty) return result;
  }
  return {
    'input_binding': _ExpertSwitchType(
      id: 'input_binding',
      name: 'Input binding',
      buttons: const ['any', 'on_press', 'off_press', 'toggle'],
      actionTypes: List.unmodifiable(actionTypes),
      defaultMapping: const {},
    ),
    'button': _ExpertSwitchType(
      id: 'button',
      name: 'Button',
      buttons: const ['any', 'on_press', 'off_press', 'toggle'],
      actionTypes: List.unmodifiable(actionTypes),
      defaultMapping: const {},
    ),
    'remote': _ExpertSwitchType(
      id: 'remote',
      name: 'Remote',
      buttons: const ['any', 'on_press', 'off_press', 'toggle'],
      actionTypes: List.unmodifiable(actionTypes),
      defaultMapping: const {},
    ),
    'rotary': _ExpertSwitchType(
      id: 'rotary',
      name: 'Rotary',
      buttons: const ['any', 'rotate'],
      actionTypes: List.unmodifiable(actionTypes),
      defaultMapping: const {},
    ),
    'motion_sensor': _ExpertSwitchType(
      id: 'motion_sensor',
      name: 'Motion sensor',
      buttons: const ['any'],
      actionTypes: List.unmodifiable(actionTypes),
      defaultMapping: const {},
    ),
    'contact_sensor': _ExpertSwitchType(
      id: 'contact_sensor',
      name: 'Contact sensor',
      buttons: const ['any'],
      actionTypes: List.unmodifiable(actionTypes),
      defaultMapping: const {},
    ),
    'camera': _ExpertSwitchType(
      id: 'camera',
      name: 'Camera',
      buttons: const ['any'],
      actionTypes: List.unmodifiable(actionTypes),
      defaultMapping: const {},
    ),
  };
}

Map<String, _ExpertSwitchmapType> _modernSwitchmapTypes(
  Map<dynamic, dynamic> switchmapRaw,
  Map<String, List<_ExpertSwitchmapAction>> actionCategories,
) {
  final mappings = _asMapOrNull(switchmapRaw['mappings']);
  if (mappings != null && mappings.isNotEmpty) {
    final result = <String, _ExpertSwitchmapType>{};
    for (final entry in mappings.entries) {
      final id = entry.key?.toString();
      final body = _asMapOrNull(entry.value);
      if (id == null || id.isEmpty || body == null) continue;
      result[id] = _ExpertSwitchmapType(
        id: id,
        name: _stringValue(body['name']) ?? _titleCase(id),
        buttons: _orderedStringList(body['buttons']),
        actionTypes: _orderedStringList(body['action_types']),
        defaultMapping: _objectMap(body['default_mapping']),
        effectiveMapping: _objectMap(body['effective_mapping']),
        hasCustom: _boolValue(body['has_custom'], fallback: false),
      );
    }
    if (result.isNotEmpty) return result;
  }

  final actionIds = <String>[
    for (final actions in actionCategories.values)
      for (final action in actions)
        if (action.id != null) action.id!,
  ];
  return {
    'input_binding': _ExpertSwitchmapType(
      id: 'input_binding',
      name: 'Input binding',
      buttons: const ['any', 'on_press', 'off_press', 'toggle'],
      actionTypes: List.unmodifiable(actionIds),
      defaultMapping: const {},
      effectiveMapping: const {},
      hasCustom: false,
    ),
  };
}

Map<String, Map<String, Object?>> _modernSwitchmapCustomMappings(
  Map<dynamic, dynamic> switchmapRaw,
) {
  final raw = _asMapOrNull(switchmapRaw['custom_mappings']);
  if (raw == null || raw.isEmpty) return const {};
  return {
    for (final entry in raw.entries)
      if (entry.key != null && _asMapOrNull(entry.value) != null)
        entry.key.toString(): _objectMap(entry.value),
  };
}

Map<String, Object?> _objectMap(Object? value) {
  final raw = _asMapOrNull(value);
  if (raw == null || raw.isEmpty) return const {};
  return {
    for (final entry in raw.entries)
      if (entry.key != null) entry.key.toString(): _jsonCompatible(entry.value),
  };
}

List<_ExpertControlSourceDevice> _controlSourceDevicesFromTopology(
  List<RhythmTopologyNode> nodes, {
  required String query,
}) {
  final normalizedQuery = query.toLowerCase();
  final nodeById = {for (final node in nodes) node.id: node};
  return [
    for (final node in nodes)
      if (_isControlSourceNode(node) &&
          (normalizedQuery.isEmpty ||
              node.name.toLowerCase().contains(normalizedQuery) ||
              node.id.toLowerCase().contains(normalizedQuery)))
        _ExpertControlSourceDevice(
          deviceId: node.id,
          name: node.name.isEmpty ? node.id : node.name,
          kind: node.kind.name,
          manufacturer: node.manufacturer,
          model: node.model,
          areaId: node.parentId,
          areaName:
              node.parentId == null ? null : nodeById[node.parentId]?.name,
          binarySensors: [
            _ExpertControlSourceSensor(
              entityId: node.id,
              name: node.name.isEmpty ? node.id : node.name,
              deviceClass: _modernDeviceClass(node),
            ),
          ],
        ),
  ];
}

bool _isControlSourceNode(RhythmTopologyNode node) {
  if (!node.isDevice || node.kind.isLightDevice) return false;
  if (node.controls.isNotEmpty) return true;
  final kind = node.kind.name;
  return kind == 'button' ||
      kind == 'switchDevice' ||
      kind == 'motionSensor' ||
      kind == 'sensor' ||
      kind == 'otherDevice';
}

String? _existingInputBindingForSource(
  Map<dynamic, dynamic> bindingsRaw, {
  required String sourceId,
  required String triggerKind,
}) {
  final bindings = bindingsRaw['bindings'];
  if (bindings is! Iterable) return null;
  for (final item in bindings) {
    final binding = _asMapOrNull(item);
    if (_stringValue(binding?['source_node_id']) != sourceId) continue;
    final trigger = _asMapOrNull(binding?['trigger']);
    if (_stringValue(trigger?['kind']) != triggerKind) continue;
    final id = _stringValue(binding?['id']);
    if (id != null && id.isNotEmpty) return id;
  }
  return null;
}

String _controlSourceBindingId(String sourceId, String triggerKind) {
  final safeSource = sourceId
      .trim()
      .toLowerCase()
      .replaceAll(RegExp(r'[^a-z0-9]+'), '-')
      .replaceAll(RegExp(r'^-+|-+$'), '');
  final sourcePart = safeSource.isEmpty ? 'source' : safeSource;
  return 'expert-input:$sourcePart:$triggerKind';
}

String _inputTriggerKindForControlSource(
  _ExpertControlSourceDevice device,
  _ExpertControlSourceSensor? sensor,
) {
  return _inputTriggerKindFromText(
        sensor?.deviceClass,
        device.kind,
        device.name,
        device.model,
        device.deviceId,
      ) ??
      'button';
}

String? _inputTriggerKindFromText(String? primary,
    [String? a, String? b, String? c, String? d]) {
  final text =
      [primary, a, b, c, d].whereType<String>().join(' ').trim().toLowerCase();
  if (text.isEmpty) return null;
  if (text.contains('motion') ||
      text.contains('presence') ||
      text.contains('occupancy')) {
    return 'motion';
  }
  if (text.contains('contact') ||
      text.contains('door') ||
      text.contains('window')) {
    return 'contact';
  }
  if (text.contains('camera')) return 'camera';
  if (text.contains('button') ||
      text.contains('switch') ||
      text.contains('remote') ||
      text.contains('rotary')) {
    return 'button';
  }
  return null;
}

String _modernControlCategory(RhythmTopologyNode? source, String? triggerKind) {
  final kind = source?.kind.name;
  return switch (triggerKind ?? kind) {
    'motion' || 'motionSensor' => 'motion_sensor',
    'contact' => 'contact_sensor',
    'camera' => 'camera',
    'button' || 'switchDevice' || 'rotary' || 'gesture' => 'switch',
    'sensor' => 'unknown',
    _ => 'switch',
  };
}

String? _modernDeviceClass(RhythmTopologyNode node) {
  return switch (node.kind.name) {
    'motionSensor' => 'motion',
    'switchDevice' || 'button' => 'button',
    'sensor' => 'sensor',
    _ => null,
  };
}

String? _modernNodeType(RhythmTopologyNode? node) {
  final kind = node?.kind.name;
  return switch (kind) {
    'motionSensor' => 'motion',
    'switchDevice' => 'switch',
    'button' => 'button',
    'sensor' => 'sensor',
    _ => kind,
  };
}

String _modernInputBindingType(
  Map<dynamic, dynamic> binding, {
  required RhythmTopologyNode? source,
  required String? triggerKind,
}) {
  return _normalizeModernInputBindingType(
        _stringValue(binding['source_type'] ?? binding['type']),
      ) ??
      _normalizeModernInputBindingType(_stringValue(binding['preset'])) ??
      _normalizeModernInputBindingType(_modernNodeType(source)) ??
      _modernInputBindingTypeForTrigger(triggerKind);
}

String _modernInputBindingTypeForTrigger(String? triggerKind) {
  return switch (triggerKind) {
    'motion' => 'motion_sensor',
    'contact' => 'contact_sensor',
    'camera' => 'camera',
    'rotary' => 'rotary',
    _ => 'button',
  };
}

String? _normalizeModernInputBindingType(String? raw) {
  final value = raw?.trim().toLowerCase();
  if (value == null || value.isEmpty) return null;
  return switch (value) {
    'motion' || 'motionsensor' || 'motion_sensor' => 'motion_sensor',
    'contact' || 'contactsensor' || 'contact_sensor' => 'contact_sensor',
    'camera' => 'camera',
    'rotary' || 'dial' => 'rotary',
    'remote' => 'remote',
    'switch' || 'switchdevice' || 'button' => 'button',
    'day_sleep_toggle' || 'input-binding' || 'input_binding' => 'input_binding',
    _ => value,
  };
}

String _modernInputBindingTypeLabel(String type, String category) {
  return switch (type) {
    'input_binding' => 'Input binding',
    'button' => 'Button',
    'remote' => 'Remote',
    'rotary' => 'Rotary',
    'motion_sensor' => 'Motion sensor',
    'contact_sensor' => 'Contact sensor',
    'camera' => 'Camera',
    _ => _controlCategoryLabel(category),
  };
}

class _ExpertScopeBindingIndex {
  final Map<String, _ExpertArea> areasById;
  final Map<String, _ExpertReachSection> sectionsById;

  const _ExpertScopeBindingIndex({
    required this.areasById,
    required this.sectionsById,
  });

  const _ExpertScopeBindingIndex.empty()
      : areasById = const {},
        sectionsById = const {};

  factory _ExpertScopeBindingIndex.fromScope(Map<dynamic, dynamic> raw) {
    final rawZones = raw['zones'];
    if (rawZones is! Iterable) return const _ExpertScopeBindingIndex.empty();

    final areas = <String, _ExpertArea>{};
    final sections = <String, _ExpertReachSection>{};
    for (final rawZone in rawZones) {
      final zone = _asMapOrNull(rawZone);
      final rawAreas = zone?['areas'];
      if (rawAreas is! Iterable) continue;
      for (final rawArea in rawAreas) {
        final area = _expertAreaFromScope(rawArea);
        if (area == null) continue;
        areas[area.id] = area;

        final areaBody = _asMapOrNull(rawArea);
        final rawSections = areaBody?['sections'];
        if (rawSections is! Iterable) continue;
        for (final rawSection in rawSections) {
          final section = _asMapOrNull(rawSection);
          final sectionId = _stringValue(section?['id']);
          if (sectionId == null || sectionId.isEmpty) continue;
          sections[sectionId] = _ExpertReachSection(
            id: sectionId,
            name: _stringValue(section?['name']) ?? sectionId,
            areaId: area.id,
            areaName: area.name,
          );
        }
      }
    }

    if (areas.isEmpty && sections.isEmpty) {
      return const _ExpertScopeBindingIndex.empty();
    }
    return _ExpertScopeBindingIndex(
      areasById: Map.unmodifiable(areas),
      sectionsById: Map.unmodifiable(sections),
    );
  }
}

_ExpertArea? _modernBindingArea(
  Map<dynamic, dynamic> binding,
  Map<String, RhythmTopologyNode> nodeById,
  RhythmTopologyNode? source, {
  required _ExpertScopeBindingIndex scopeIndex,
}) {
  for (final targetId in _modernBindingTargetIds(binding)) {
    final area = scopeIndex.areasById[targetId];
    if (area != null) return area;

    final section = scopeIndex.sectionsById[targetId];
    if (section != null) {
      return scopeIndex.areasById[section.areaId] ??
          _ExpertArea(
            id: section.areaId,
            name: section.areaName,
            deviceCount: 0,
            brightnessOffset: 0,
          );
    }

    final target = nodeById[targetId];
    if (target == null) continue;
    if (target.kind.isRoom) {
      return _ExpertArea(
        id: target.id,
        name: target.name,
        deviceCount: 0,
        brightnessOffset: 0,
      );
    }
    final parent = target.parentId == null ? null : nodeById[target.parentId];
    if (parent != null && parent.kind.isRoom) {
      return _ExpertArea(
        id: parent.id,
        name: parent.name,
        deviceCount: 0,
        brightnessOffset: 0,
      );
    }
  }
  final parent = source?.parentId == null ? null : nodeById[source!.parentId];
  if (parent == null || !parent.kind.isRoom) return null;
  return _ExpertArea(
    id: parent.id,
    name: parent.name,
    deviceCount: 0,
    brightnessOffset: 0,
  );
}

List<_ExpertSwitchScope> _modernBindingScopes(
  Map<dynamic, dynamic> binding,
  Map<String, RhythmTopologyNode> nodeById,
  RhythmTopologyNode? source, {
  required _ExpertScopeBindingIndex scopeIndex,
}) {
  final scopedTargetLists = _modernBindingTargetListScopes(
    binding,
    nodeById,
    scopeIndex: scopeIndex,
  );
  if (scopedTargetLists.isNotEmpty) return scopedTargetLists;

  final targetIds = _modernBindingTargetIds(binding);
  final areaIds = <String>[];
  final sectionIds = <String>[];
  void addArea(String areaId) {
    if (!areaIds.contains(areaId)) areaIds.add(areaId);
  }

  void addSection(String sectionId) {
    if (!sectionIds.contains(sectionId)) sectionIds.add(sectionId);
  }

  for (final targetId in targetIds) {
    if (scopeIndex.areasById.containsKey(targetId)) {
      addArea(targetId);
      continue;
    }

    final section = scopeIndex.sectionsById[targetId];
    if (section != null) {
      addSection(section.id);
      continue;
    }

    final target = nodeById[targetId];
    if (target == null) continue;
    if (target.kind.isRoom) {
      addArea(target.id);
    } else if (target.kind.isLightDevice) {
      addSection(target.id);
      final parentId = target.parentId;
      if (parentId != null) addArea(parentId);
    }
  }
  final disabledWithoutTargets =
      targetIds.isEmpty && !_boolValue(binding['enabled'], fallback: true);
  if (areaIds.isEmpty && source?.parentId != null && !disabledWithoutTargets) {
    addArea(source!.parentId!);
  }
  if (areaIds.isEmpty && sectionIds.isEmpty) return const [];
  return [
    _ExpertSwitchScope(
      areaIds: areaIds,
      sectionIds: sectionIds,
      feedbackArea: areaIds.isEmpty ? null : areaIds.first,
      mode: _sensorModeFromInputBindingAction(
        _inputBindingActionId(binding['action']),
        hasTimer: _nullableInt(binding['auto_off_secs']) != null,
      ),
      durationSeconds: _intValue(binding['auto_off_secs'], fallback: 0),
      triggerEntities: [
        if (_stringValue(binding['source_node_id']) case final String sourceId)
          sourceId,
      ],
    ),
  ];
}

List<_ExpertSwitchScope> _modernBindingTargetListScopes(
  Map<dynamic, dynamic> binding,
  Map<String, RhythmTopologyNode> nodeById, {
  required _ExpertScopeBindingIndex scopeIndex,
}) {
  final targetLists = binding['target_lists'];
  if (targetLists is! Iterable) return const [];
  final scopes = <_ExpertSwitchScope>[];
  for (final item in targetLists) {
    final list = _asMapOrNull(item);
    if (list == null) continue;
    final targetIds = _modernBindingTargetIdsFromRaw(list['targets']);
    if (targetIds.isEmpty) continue;
    final areaIds = <String>[];
    final sectionIds = <String>[];
    void addArea(String areaId) {
      if (!areaIds.contains(areaId)) areaIds.add(areaId);
    }

    void addSection(String sectionId) {
      if (!sectionIds.contains(sectionId)) sectionIds.add(sectionId);
    }

    for (final targetId in targetIds) {
      if (scopeIndex.areasById.containsKey(targetId)) {
        addArea(targetId);
        continue;
      }
      final section = scopeIndex.sectionsById[targetId];
      if (section != null) {
        addSection(section.id);
        continue;
      }
      final target = nodeById[targetId];
      if (target == null) continue;
      if (target.kind.isRoom) {
        addArea(target.id);
      } else if (target.kind.isLightDevice) {
        addSection(target.id);
        final parentId = target.parentId;
        if (parentId != null) addArea(parentId);
      }
    }
    if (areaIds.isEmpty && sectionIds.isEmpty) continue;
    scopes.add(
      _ExpertSwitchScope(
        areaIds: areaIds,
        sectionIds: sectionIds,
        feedbackArea: _stringValue(list['feedback_area']) ??
            (areaIds.isEmpty ? null : areaIds.first),
        mode: _sensorModeFromInputBindingAction(_stringValue(list['mode'])),
        durationSeconds: _intValue(
          list['duration'] ?? list['duration_secs'],
          fallback: 60,
        ),
        cooldownSeconds: _intValue(
          list['cooldown'] ?? list['cooldown_secs'],
          fallback: 0,
        ),
        boostEnabled: _boolValue(list['boost_enabled'], fallback: false),
        boostBrightness: _intValue(
          list['boost_brightness'],
          fallback: 50,
        ),
        alertIntensity: _stringValue(list['alert_intensity']) ?? 'low',
        alertCount: _intValue(list['alert_count'], fallback: 3),
        activeWhen: _stringValue(list['active_when']) ?? 'always',
        activeOffset: _intValue(list['active_offset'], fallback: 0),
        triggerEntities: _orderedStringList(list['trigger_entities']),
      ),
    );
  }
  return scopes;
}

String _sensorModeFromInputBindingAction(String? actionId,
    {bool hasTimer = false}) {
  if (actionId == 'on_only' ||
      actionId == 'on_off' ||
      actionId == 'alert' ||
      actionId == 'disabled') {
    return actionId!;
  }
  if (actionId == 'circadian_on') return hasTimer ? 'on_off' : 'on_only';
  if (actionId == 'circadian_off') return 'disabled';
  return 'on_off';
}

List<String> _modernBindingTargetIds(Map<dynamic, dynamic> binding) {
  final ids = <String>[];

  final directTargets = binding['targets'];
  ids.addAll(_modernBindingTargetIdsFromRaw(directTargets));
  final targetLists = binding['target_lists'];
  if (targetLists is Iterable) {
    for (final list in targetLists) {
      final targets = _asMapOrNull(list)?['targets'];
      ids.addAll(_modernBindingTargetIdsFromRaw(targets));
    }
  }
  return _uniqueOrdered(ids);
}

List<String> _modernBindingTargetIdsFromRaw(Object? raw) {
  if (raw is! Iterable) return const [];
  final ids = <String>[];
  for (final target in raw) {
    final body = _asMapOrNull(target);
    final id = _stringValue(
      body?['id'] ?? body?['node_id'] ?? body?['area_id'] ?? target,
    );
    if (id != null && !ids.contains(id)) ids.add(id);
  }
  return ids;
}

Map<String, Object?> _modernInputBindingPayloadFromControl(
  _ExpertControl control,
  Map<dynamic, dynamic> binding,
) {
  final payload = _modernInputBindingBasePayload(binding);
  final targets = _inputBindingTargetIdsFromScopes(control.scopes);
  final targetLists = _inputBindingTargetListsFromScopes(control.scopes);
  payload['name'] = control.name;
  payload['source_type'] = control.type;
  payload['source_node_id'] = _inputBindingSourceNodeId(
    binding,
    fallback: control.deviceId,
    scopes: control.scopes,
  );
  payload['trigger'] = _inputBindingTriggerPayload(
    binding,
    fallbackKind: _modernInputTriggerKindForCategory(control.category),
  );
  payload['action'] = _jsonCompatible(binding['action']) ??
      _catalogInputBindingActionPayload(
        'circadian_on',
        control.scopes,
      );
  payload['targets'] = targets;
  payload['target_lists'] = targetLists;
  payload['enabled'] = !control.inactive || targets.isNotEmpty;
  _applyInputBindingAutoOff(payload, binding, control.scopes);
  return payload;
}

Map<String, Object?> _modernInputBindingPayloadFromSwitch(
  _ExpertSwitch control,
  Map<dynamic, dynamic> binding,
) {
  final payload = _modernInputBindingBasePayload(binding);
  final targets = _inputBindingTargetIdsFromScopes(control.scopes);
  final triggerEvent = _inputBindingTriggerEvent(binding);
  final rawAction = _asMapOrNull(binding['action']);
  final rawActionKind = _stringValue(rawAction?['kind']);
  final currentActionId = _inputBindingActionId(binding['action']);
  final selectedAction = control.magicButtons[triggerEvent] ??
      (control.magicButtons.length == 1
          ? control.magicButtons.values.first
          : null);
  final removedCurrentMoment =
      !control.magicButtons.containsKey(triggerEvent) &&
          _isDynamicMomentActionId(currentActionId);

  payload['name'] = control.name;
  payload['source_type'] = control.type;
  payload['source_node_id'] = _inputBindingSourceNodeId(
    binding,
    fallback: control.deviceId,
    scopes: control.scopes,
  );
  payload['trigger'] = _inputBindingTriggerPayload(
    binding,
    fallbackKind: 'button',
  );
  payload['targets'] = targets;
  payload['target_lists'] = const <Object?>[];
  payload['enabled'] =
      (!control.inactive || targets.isNotEmpty) && !removedCurrentMoment;
  payload['action'] = selectedAction != null &&
          selectedAction.isNotEmpty &&
          (rawActionKind == 'catalog' ||
              _isDynamicMomentActionId(selectedAction))
      ? _catalogInputBindingActionPayload(selectedAction, control.scopes)
      : _jsonCompatible(binding['action']) ??
          _catalogInputBindingActionPayload('circadian_on', control.scopes);
  _applyInputBindingAutoOff(payload, binding, control.scopes);
  return payload;
}

Map<String, Object?> _modernInputBindingResetPayload(
  Map<dynamic, dynamic> binding,
) {
  final payload = _modernInputBindingBasePayload(binding);
  payload['source_node_id'] = _stringValue(binding['source_node_id']);
  payload['trigger'] =
      _jsonCompatible(binding['trigger']) ?? const {'kind': 'button'};
  payload['action'] = _jsonCompatible(binding['action']) ??
      _catalogInputBindingActionPayload('circadian_on', const []);
  payload['targets'] = const <String>[];
  payload['target_lists'] = const <Object?>[];
  payload['enabled'] = false;
  payload.remove('auto_off_secs');
  return payload;
}

Map<String, Object?> _modernInputBindingBasePayload(
  Map<dynamic, dynamic> binding,
) {
  final payload = <String, Object?>{};
  for (final entry in binding.entries) {
    final key = entry.key?.toString();
    if (key == null || key.isEmpty) continue;
    payload[key] = _jsonCompatible(entry.value);
  }
  return payload;
}

Object? _jsonCompatible(Object? value) {
  if (value is Map) {
    return {
      for (final entry in value.entries)
        if (entry.key != null)
          entry.key.toString(): _jsonCompatible(entry.value),
    };
  }
  if (value is Iterable && value is! String) {
    return [for (final item in value) _jsonCompatible(item)];
  }
  return value;
}

String _inputBindingSourceNodeId(
  Map<dynamic, dynamic> binding, {
  required String? fallback,
  required List<_ExpertSwitchScope> scopes,
}) {
  for (final scope in scopes) {
    for (final entityId in scope.triggerEntities) {
      final trimmed = entityId.trim();
      if (trimmed.isNotEmpty) return trimmed;
    }
  }
  return fallback ?? _stringValue(binding['source_node_id']) ?? '';
}

Map<String, Object?> _inputBindingTriggerPayload(
  Map<dynamic, dynamic> binding, {
  required String fallbackKind,
}) {
  final trigger = _asMapOrNull(binding['trigger']);
  if (trigger == null) return {'kind': fallbackKind};
  return {
    'kind': _stringValue(trigger['kind']) ?? fallbackKind,
    if (_stringValue(trigger['button_action']) case final buttonAction?)
      'button_action': buttonAction,
  };
}

String _modernInputTriggerKindForCategory(String category) {
  return switch (category) {
    'motion_sensor' => 'motion',
    'contact_sensor' => 'contact',
    'camera' => 'camera',
    _ => 'button',
  };
}

String _inputBindingTriggerEvent(Map<dynamic, dynamic> binding) {
  final trigger = _asMapOrNull(binding['trigger']);
  return _stringValue(trigger?['button_action']) ?? 'any';
}

List<String> _inputBindingTargetIdsFromScopes(
  List<_ExpertSwitchScope> scopes,
) {
  final ids = <String>[];
  void add(String id) {
    final trimmed = id.trim();
    if (trimmed.isNotEmpty && !ids.contains(trimmed)) ids.add(trimmed);
  }

  for (final scope in scopes) {
    for (final areaId in scope.areaIds) {
      add(areaId);
    }
    for (final sectionId in scope.sectionIds) {
      add(sectionId);
    }
  }
  return ids;
}

List<Map<String, Object?>> _inputBindingTargetListsFromScopes(
  List<_ExpertSwitchScope> scopes,
) {
  final lists = <Map<String, Object?>>[];
  for (var index = 0; index < scopes.length; index += 1) {
    final scope = scopes[index];
    final targets = _inputBindingTargetIdsFromScopes([scope]);
    if (targets.isEmpty) continue;
    lists.add({
      'id': 'reach-${index + 1}',
      'targets': targets,
      if (scope.feedbackArea != null) 'feedback_area': scope.feedbackArea,
      'mode': scope.mode,
      'duration': scope.durationSeconds,
      'cooldown': scope.cooldownSeconds,
      'boost_enabled': scope.boostEnabled,
      'boost_brightness': scope.boostBrightness,
      'active_when': scope.activeWhen,
      'active_offset': scope.activeOffset,
      'trigger_entities': scope.triggerEntities,
      'alert_intensity': scope.alertIntensity,
      'alert_count': scope.alertCount,
    });
  }
  return lists;
}

Map<String, Object?> _catalogInputBindingActionPayload(
  String actionId,
  List<_ExpertSwitchScope> scopes,
) {
  return {
    'kind': 'catalog',
    'source_action_id': actionId,
    'target_grain': _inputBindingTargetGrainForAction(actionId, scopes),
  };
}

String _inputBindingTargetGrainForAction(
  String actionId,
  List<_ExpertSwitchScope> scopes,
) {
  if (_isDynamicMomentActionId(actionId)) return 'global';
  const zoneActions = {
    'glo_up',
    'full_send',
    'glozone_down',
    'reset_zone',
    'reset_zone_cascade',
    'glozone_reset',
    'glozone_reset_full',
    'glozone_reset_brightness_override',
    'glozone_reset_color_override',
    'glozone_reset_phase',
    'glozone_reset_frozen',
    'sun_up',
    'sun_down',
  };
  if (zoneActions.contains(actionId)) return 'zone';
  final hasAreaTargets = scopes.any((scope) => scope.areaIds.isNotEmpty);
  final hasSectionTargets = scopes.any((scope) => scope.sectionIds.isNotEmpty);
  if (hasSectionTargets && !hasAreaTargets) return 'section';
  return 'room';
}

void _applyInputBindingAutoOff(
  Map<String, Object?> payload,
  Map<dynamic, dynamic> binding,
  List<_ExpertSwitchScope> scopes,
) {
  final actionId = _inputBindingActionId(payload['action']) ??
      _inputBindingActionId(binding['action']);
  if (!_isDynamicMomentActionId(actionId)) {
    payload.remove('auto_off_secs');
    return;
  }
  final duration = scopes
      .map((scope) => scope.durationSeconds)
      .where((seconds) => seconds > 0)
      .cast<int?>()
      .firstOrNull;
  payload['auto_off_secs'] = duration ?? _nullableInt(binding['auto_off_secs']);
}

bool _isDynamicMomentActionId(String? actionId) {
  if (actionId == null || !actionId.startsWith('set_')) return false;
  const staticSetActions = {
    'set_britelite',
    'set_nitelite',
    'set_wake_or_bed',
    'set_wake',
    'set_bed',
    'set_position_step',
    'set_position_brightness',
    'set_position_color',
  };
  return !staticSetActions.contains(actionId);
}

String? _inputBindingActionId(Object? raw) {
  final action = _asMapOrNull(raw);
  if (action == null) return null;
  final kind = _stringValue(action['kind']);
  if (kind == 'catalog') {
    return _stringValue(
      action['source_action_id'] ??
          _asMapOrNull(action['compiled'])?['source_id'] ??
          _asMapOrNull(action['compiled'])?['canonical_id'],
    );
  }
  if (kind == 'unsupported_imported' || kind == 'section_error_bounce') {
    return _stringValue(action['source_action_id']);
  }
  if (kind == 'mode_cycle' || kind == 'mode_set' || kind == 'mode_toggle') {
    return kind;
  }
  return kind;
}

bool _inputBindingActionIsUnsupportedImported(Object? raw) {
  return _stringValue(_asMapOrNull(raw)?['kind']) == 'unsupported_imported';
}

List<_ExpertZone> _zonesWithCurrent(
  List<_ExpertZone> zones,
  _ExpertZone current,
) {
  var found = false;
  final next = <_ExpertZone>[];
  for (final zone in zones) {
    if (zone.id == current.id) {
      next.add(current);
      found = true;
    } else {
      next.add(zone);
    }
  }
  if (!found) next.add(current);

  final deduped = <_ExpertZone>[];
  final seen = <String>{};
  for (final zone in next) {
    if (seen.add(zone.id)) deduped.add(zone);
  }
  return deduped;
}

List<_ExpertZone> _demoZones() {
  return const [
    _ExpertZone(
      name: 'Main floor',
      isDefault: true,
      areas: [
        _ExpertArea(
          id: 'area.kitchen',
          name: 'Kitchen',
          deviceCount: 5,
          brightnessOffset: 0,
        ),
        _ExpertArea(
          id: 'area.living_room',
          name: 'Living room',
          deviceCount: 7,
          brightnessOffset: -4,
        ),
        _ExpertArea(
          id: 'area.dining',
          name: 'Dining',
          deviceCount: 3,
          brightnessOffset: 2,
        ),
      ],
      definition: _RhythmDefinition.defaults(),
    ),
    _ExpertZone(
      name: 'Bedrooms',
      isDefault: false,
      areas: [
        _ExpertArea(
          id: 'area.primary_bedroom',
          name: 'Primary bedroom',
          deviceCount: 4,
          brightnessOffset: -12,
        ),
        _ExpertArea(
          id: 'area.guest_room',
          name: 'Guest room',
          deviceCount: 2,
          brightnessOffset: -8,
        ),
      ],
      definition: _RhythmDefinition(
        sleepPattern: 'early',
        wakeHour: 5.75,
        bedHour: 21.75,
        minBrightness: 12,
        maxBrightness: 76,
        sleepBrightness: 4,
        minKelvin: 2000,
        maxKelvin: 4600,
        daylightDimming: 0.42,
        naturalExposure: 0.5,
        transitionMinutes: 45,
        phaseBalance: -0.2,
      ),
    ),
    _ExpertZone(
      name: 'Studio',
      isDefault: false,
      areas: [
        _ExpertArea(
          id: 'area.desk',
          name: 'Desk',
          deviceCount: 2,
          brightnessOffset: 8,
        ),
        _ExpertArea(
          id: 'area.work_bench',
          name: 'Work bench',
          deviceCount: 3,
          brightnessOffset: 12,
        ),
      ],
      definition: _RhythmDefinition(
        sleepPattern: 'late',
        wakeHour: 8,
        bedHour: 0.5,
        minBrightness: 22,
        maxBrightness: 100,
        sleepBrightness: 10,
        minKelvin: 2400,
        maxKelvin: 6000,
        daylightDimming: 0.2,
        naturalExposure: 0.35,
        transitionMinutes: 25,
        phaseBalance: 0.3,
      ),
    ),
  ];
}

Map<String, Object?> _expertConfigValuesFromServer(Map<dynamic, dynamic> raw) {
  final values = <String, Object?>{};
  for (final entry in raw.entries) {
    final key = _stringValue(entry.key);
    if (key != null) values[key] = entry.value;
  }
  return values;
}

Map<String, Object?> _expertSettingsValuesFromServer(
  Map<dynamic, dynamic> raw,
) {
  final values = <String, Object?>{};
  for (final key in [
    'auto_update',
    'power_recovery',
    'turn_on_transition',
    'turn_off_transition',
    'multi_click_enabled',
    'multi_click_speed',
    'environment_chain',
    'multi_area_dispatch_stagger_ms',
    'experimental_tick_mode',
    'long_press_repeat_interval',
    'motion_blink_threshold',
    'motion_warning_time',
    'off_threshold',
    'daily_sync_hour',
    'daily_sync_minute',
    'read_only_zha',
    'feedback_restrict_to_primary',
    'reach_feedback_enabled',
    'reach_daytime_threshold',
    'freeze_feedback_enabled',
    'limit_bounce_enabled',
    'limit_bounce_max_percent',
    'limit_bounce_min_percent',
    'sun_saturation',
    'boost_default',
    'confirm_zone_pushes',
    'duration_picker_presets',
    'default_pause_duration_minutes',
    'default_freeze_duration_minutes',
    'default_boost_duration_minutes',
    'default_power_off_duration_minutes',
    'rhythm_cursor_step_min',
    'controls_pulse_window_hours',
    'controls_recent_window_minutes',
    'activity_log_min_entries',
    'activity_log_min_days',
    'activity_log_flush_interval_minutes',
    'tick_repeat_after_user_action',
    'tick_repeat_after_autonomous_change',
    'periodic_refresh_interval_minutes',
  ]) {
    if (raw.containsKey(key)) values[key] = raw[key];
  }

  for (final key in const [
    'boost_return_transition',
    'freeze_hold_at_dim',
    'freeze_off_rise',
    'limit_warning_speed',
    'alert_bounce_speed',
  ]) {
    final ms = _nullableDouble(raw[key]);
    if (ms != null) values[key] = ms / 100;
  }

  final ctCompensation = _asMapOrNull(raw['ct_compensation']);
  if (ctCompensation != null) {
    values['ct_comp_enabled'] = ctCompensation['enabled'];
    values['ct_comp_begin'] = ctCompensation['begin_kelvin'];
    values['ct_comp_end'] = ctCompensation['end_kelvin'];
    values['ct_comp_factor'] = ctCompensation['factor'];
  }

  final twoStep = _asMapOrNull(raw['two_step_turn_on']);
  if (twoStep != null) {
    values['two_step_enabled'] = twoStep['enabled'];
    values['two_step_ct_threshold'] = twoStep['kelvin_threshold'];
    values['two_step_bri_threshold'] = twoStep['brightness_threshold_percent'];
    final delayMs = _nullableDouble(twoStep['delay_ms']);
    if (delayMs != null) values['two_step_delay'] = delayMs / 100;
  }

  final solarRules = _asMapOrNull(raw['solar_color_rules']);
  if (solarRules != null) {
    for (final key in [
      'warm_night_enabled',
      'warm_night_mode',
      'warm_night_target',
      'daylight_enabled',
      'daylight_cct',
      'color_sensitivity',
    ]) {
      if (solarRules.containsKey(key)) values[key] = solarRules[key];
    }
    for (final entry in const {
      'warm_night_start': 'warm_night_start_minutes',
      'warm_night_end': 'warm_night_end_minutes',
      'warm_night_fade': 'warm_night_fade_minutes',
      'daylight_start': 'daylight_start_minutes',
      'daylight_end': 'daylight_end_minutes',
      'daylight_fade': 'daylight_fade_minutes',
    }.entries) {
      if (solarRules.containsKey(entry.value)) {
        values[entry.key] = solarRules[entry.value];
      } else if (solarRules.containsKey(entry.key)) {
        values[entry.key] = solarRules[entry.key];
      }
    }
  }

  return values;
}

@visibleForTesting
Map<String, Object?> expertSettingsValuesFromServerForTest(
  Map<dynamic, dynamic> raw,
) =>
    _expertSettingsValuesFromServer(raw);

@visibleForTesting
MapEntry<String, Object?> modernExpertSettingsEntryForTest(
  String key,
  Object? value,
) =>
    _modernExpertSettingsEntry(key, value);

@visibleForTesting
List<String> durationPresetValuesForTest(
  Map<String, Object?> values, {
  Iterable<String> includeValues = const [],
}) =>
    _durationPresetValues(
      _ExpertConfig(values),
      includeValues: includeValues,
    );

@visibleForTesting
List<String> positiveDurationPresetValuesForTest(
  Iterable<String> values, {
  Iterable<String> includeValues = const [],
}) =>
    _positiveDurationPresetValues(values, includeValues: includeValues);

@visibleForTesting
int expertDurationMinutesForTest(
  Map<String, Object?> values,
  String key, {
  required int fallback,
}) =>
    _expertDurationMinutes(_ExpertConfig(values), key, fallback: fallback);

@visibleForTesting
int expertCursorStepMinutesForTest(Map<String, Object?> values) =>
    _expertCursorStepMinutes(_ExpertConfig(values));

@visibleForTesting
double nudgeExpertPhaseHourForTest(
  double hour,
  int direction,
  int stepMinutes,
) =>
    _nudgeExpertPhaseHour(hour, direction, stepMinutes);

@visibleForTesting
int expertControlsPulseWindowHoursForTest(Map<String, Object?> values) =>
    _expertControlsPulseWindowHours(_ExpertConfig(values));

@visibleForTesting
int expertControlsRecentWindowMinutesForTest(Map<String, Object?> values) =>
    _expertControlsRecentWindowMinutes(_ExpertConfig(values));

@visibleForTesting
String controlPulseStateForTest(
  DateTime? lastActionTime,
  DateTime now,
  int pulseWindowHours,
  int recentWindowMinutes,
) =>
    _controlPulseState(
      lastActionTime,
      now,
      pulseWindowHours,
      recentWindowMinutes,
    ).name;

MapEntry<String, Object?> _modernExpertSettingsEntry(
  String key,
  Object? value,
) {
  return switch (key) {
    'ct_comp_enabled' => MapEntry('ct_compensation_enabled', value),
    'ct_comp_begin' => MapEntry('ct_compensation_begin_kelvin', value),
    'ct_comp_end' => MapEntry('ct_compensation_end_kelvin', value),
    'ct_comp_factor' => MapEntry('ct_compensation_factor', value),
    'two_step_ct_threshold' => MapEntry('two_step_kelvin_threshold', value),
    'two_step_bri_threshold' => MapEntry(
        'two_step_brightness_threshold',
        value,
      ),
    'two_step_delay' => MapEntry(
        'two_step_delay_ms',
        _twoStepDelayMsFromTenths(value),
      ),
    'freeze_hold_at_dim' => MapEntry(key, _millisecondsFromTenths(value)),
    'freeze_off_rise' => MapEntry(key, _millisecondsFromTenths(value)),
    'limit_warning_speed' => MapEntry(key, _millisecondsFromTenths(value)),
    'alert_bounce_speed' => MapEntry(key, _millisecondsFromTenths(value)),
    'boost_return_transition' => MapEntry(key, _millisecondsFromTenths(value)),
    _ => MapEntry(key, value),
  };
}

Object? _twoStepDelayMsFromTenths(Object? value) {
  final tenths = _nullableDouble(value);
  if (tenths == null) return value;
  return (tenths * 100).round();
}

Object? _millisecondsFromTenths(Object? value) {
  final tenths = _nullableDouble(value);
  if (tenths == null) return value;
  return (tenths * 100).round();
}

Map<dynamic, dynamic> _asMap(Object? value) {
  if (value is Map) return value;
  throw FormatException('Expected JSON object, got ${value.runtimeType}');
}

Map<dynamic, dynamic>? _asMapOrNull(Object? value) {
  return value is Map ? value : null;
}

Map<String, dynamic> _stringMap(Object? value) {
  final map = _stringMapOrNull(value);
  if (map != null) return map;
  throw FormatException('Expected JSON object, got ${value.runtimeType}');
}

Map<String, dynamic>? _stringMapOrNull(Object? value) {
  if (value is Map<String, dynamic>) return value;
  if (value is! Map) return null;
  return value.map((key, value) => MapEntry(key.toString(), value));
}

Map<dynamic, dynamic>? _hardwareCapability(
  Map<dynamic, dynamic> raw,
  String key,
) {
  final capabilities = raw['capabilities'];
  if (capabilities is! Iterable) return null;
  for (final capability in capabilities) {
    final body = _asMapOrNull(capability);
    if (_stringValue(body?['key']) == key) return body;
  }
  return null;
}

String? _stringValue(Object? value) {
  if (value == null) return null;
  final text = value.toString().trim();
  return text.isEmpty ? null : text;
}

int _intValue(Object? value, {required int fallback}) {
  if (value is int) return value;
  if (value is num) return value.round();
  return int.tryParse(value?.toString() ?? '') ?? fallback;
}

int? _nullableInt(Object? value) {
  if (value == null) return null;
  if (value is int) return value;
  if (value is num) return value.round();
  return int.tryParse(value.toString());
}

String? _isoFromEpochMs(Object? value) {
  final epochMs = _nullableInt(value);
  if (epochMs == null) return null;
  return DateTime.fromMillisecondsSinceEpoch(
    epochMs,
    isUtc: true,
  ).toIso8601String();
}

List<int> _intListValue(Object? value, {required List<int> fallback}) {
  if (value is! Iterable) return List<int>.from(fallback);
  final values = <int>[];
  for (final item in value) {
    final parsed = _intValue(item, fallback: -1);
    if (parsed >= 0 && parsed <= 6 && !values.contains(parsed)) {
      values.add(parsed);
    }
  }
  values.sort();
  return values;
}

double _doubleValue(Object? value, {required double fallback}) {
  if (value is num) return value.toDouble();
  return double.tryParse(value?.toString() ?? '') ?? fallback;
}

double? _nullableDouble(Object? value) {
  if (value == null) return null;
  if (value is num) return value.toDouble();
  return double.tryParse(value.toString());
}

List<String> _stringList(Object? value) {
  if (value is! Iterable) return const [];
  final values = <String>[];
  for (final item in value) {
    final text = _stringValue(item);
    if (text != null && !values.contains(text)) values.add(text);
  }
  values.sort();
  return values;
}

List<String> _orderedStringList(Object? value) {
  if (value is! Iterable) return const [];
  final values = <String>[];
  for (final item in value) {
    final text = _stringValue(item);
    if (text != null && !values.contains(text)) values.add(text);
  }
  return values;
}

bool _boolValue(Object? value, {required bool fallback}) {
  if (value is bool) return value;
  if (value is num) return value != 0;
  final text = value?.toString().trim().toLowerCase();
  return switch (text) {
    'true' || 'yes' || 'on' || '1' => true,
    'false' || 'no' || 'off' || '0' => false,
    _ => fallback,
  };
}

List<String> _optionsWithCurrent(String current, List<String> options) {
  if (options.contains(current)) return options;
  return [current, ...options];
}

num _steppedConfigNumber(
  num candidate, {
  required num min,
  required num max,
  required num step,
  required bool integer,
}) {
  final safeStep = step <= 0 ? 1 : step;
  final rounded = min + (((candidate - min) / safeStep).round() * safeStep);
  final clamped = rounded.clamp(min, max);
  if (integer) return clamped.round();

  final decimals = safeStep.toDouble() < 0.1
      ? 2
      : safeStep.toDouble() < 1
          ? 1
          : 0;
  return double.parse(clamped.toDouble().toStringAsFixed(decimals));
}

String _formatConfigNumber(num value) {
  if (value is int || value.toDouble() == value.roundToDouble()) {
    return value.round().toString();
  }
  return value.toStringAsFixed(2).replaceFirst(RegExp(r'\.?0+$'), '');
}

String _formatConfigTime(int hour, int minute) {
  final safeHour = hour.clamp(0, 23).toInt();
  final safeMinute = minute.clamp(0, 59).toInt();
  final period = safeHour >= 12 ? 'PM' : 'AM';
  final displayHour = safeHour % 12 == 0 ? 12 : safeHour % 12;
  return '$displayHour:${safeMinute.toString().padLeft(2, '0')} $period';
}

String _durationOptionLabel(String value) {
  return switch (value) {
    '0' => 'Forever',
    '5' => '5 min',
    '60' => '1 hour',
    '240' => '4 hours',
    '1440' => 'Day',
    '10080' => 'Week',
    _ => '$value min',
  };
}

List<String> _durationPresetValues(
  _ExpertConfig config, {
  Iterable<String> includeValues = const [],
}) {
  final values = <String>[];

  void addValue(Object? raw) {
    final normalized = _durationPresetValue(raw);
    if (normalized == null || values.contains(normalized)) return;
    values.add(normalized);
  }

  final raw = config.values['duration_picker_presets'];
  if (raw is String) {
    for (final token in raw.split(',')) {
      addValue(token);
    }
  } else if (raw is Iterable) {
    for (final value in raw) {
      addValue(value);
    }
  } else if (raw is Map) {
    final rawValues = raw['values'];
    final rawLen = _nullableInt(raw['len']);
    if (rawValues is Iterable) {
      final limit = rawLen == null
          ? rawValues.length
          : rawLen.clamp(0, rawValues.length).toInt();
      for (final value in rawValues.take(limit)) {
        addValue(value);
      }
    }
  }

  if (values.isEmpty) {
    values.addAll(const ['5', '60', '240', '1440', '10080', '0']);
  }
  for (final value in includeValues) {
    addValue(value);
  }
  return List.unmodifiable(values);
}

List<String> _positiveDurationPresetValues(
  Iterable<String> values, {
  Iterable<String> includeValues = const [],
}) {
  final positive = <String>[];

  void addValue(Object? raw) {
    final normalized = _durationPresetValue(raw);
    if (normalized == null || normalized == '0') return;
    if (positive.contains(normalized)) return;
    positive.add(normalized);
  }

  for (final value in values) {
    addValue(value);
  }
  for (final value in includeValues) {
    addValue(value);
  }
  if (positive.isEmpty) {
    positive.addAll(const ['5', '60', '240']);
  }
  return List.unmodifiable(positive);
}

int _expertDurationMinutes(
  _ExpertConfig config,
  String key, {
  required int fallback,
}) {
  return config.intValue(key, fallback: fallback).clamp(0, 10080).toInt();
}

int _expertCursorStepMinutes(_ExpertConfig config) {
  return config
      .intValue('rhythm_cursor_step_min', fallback: 5)
      .clamp(1, 60)
      .toInt();
}

int _expertControlsPulseWindowHours(_ExpertConfig config) {
  return config
      .intValue('controls_pulse_window_hours', fallback: 6)
      .clamp(1, 168)
      .toInt();
}

int _expertControlsRecentWindowMinutes(_ExpertConfig config) {
  return config
      .intValue('controls_recent_window_minutes', fallback: 5)
      .clamp(1, 1440)
      .toInt();
}

enum _ControlPulseState { none, pulse, recent }

_ControlPulseState _controlPulseState(
  DateTime? lastActionTime,
  DateTime now,
  int pulseWindowHours,
  int recentWindowMinutes,
) {
  if (lastActionTime == null) return _ControlPulseState.none;
  final age = now.difference(lastActionTime);
  final clampedAge = age.isNegative ? Duration.zero : age;
  final recentWindow = Duration(
    minutes: recentWindowMinutes.clamp(1, 1440).toInt(),
  );
  if (clampedAge <= recentWindow) return _ControlPulseState.recent;
  final pulseWindow = Duration(
    hours: pulseWindowHours.clamp(1, 168).toInt(),
  );
  if (clampedAge <= pulseWindow) return _ControlPulseState.pulse;
  return _ControlPulseState.none;
}

double _nudgeExpertPhaseHour(double hour, int direction, int stepMinutes) {
  final clampedStep = stepMinutes.clamp(1, 60).toInt();
  final sign = direction < 0 ? -1 : 1;
  final next = _normalizeHour(hour + sign * clampedStep / 60);
  return (next * 60).round() / 60;
}

String? _durationPresetValue(Object? raw) {
  if (raw is String) {
    final trimmed = raw.trim();
    if (trimmed.isEmpty) return null;
    if (trimmed.toLowerCase() == 'forever') return '0';
    final parsed = int.tryParse(trimmed);
    if (parsed == null || parsed < 0) return null;
    return parsed.toString();
  }
  final parsed = _nullableInt(raw);
  if (parsed == null || parsed < 0) return null;
  return parsed.toString();
}

String _outdoorSourceLabel(String source) {
  return switch (source) {
    'manual_override' => 'Override',
    'lux_sensor' => 'Sensor',
    'sun_angle_fallback' => 'Sun',
    'lux' => 'Sensor',
    'angle' => 'Sun',
    'weather' => 'Weather',
    'override' => 'Override',
    _ => _titleCase(source),
  };
}

String _outdoorConditionLabel(
  String condition,
  _ExpertOutdoorStatus? status,
) {
  final fallback = switch (condition) {
    'clear' => 'Clear',
    'partly_cloudy' => 'Partly cloudy',
    'partlycloudy' => 'Partly cloudy',
    'overcast' => 'Overcast',
    'rain' => 'Rain',
    'storm' => 'Storm',
    'snowy' => 'Snow',
    'lightning' => 'Storm',
    _ => _titleCase(condition.replaceAll('_', ' ')),
  };
  if (status == null) return fallback;
  for (final group in status.weatherGroups) {
    if (group.key != condition) continue;
    return '${group.label} ${(group.multiplier * 100).round()}%';
  }
  return fallback;
}

String _modernSkyCondition(String condition) {
  return switch (condition) {
    'sunny' || 'clear-night' => 'clear',
    'partlycloudy' => 'partly_cloudy',
    'rainy' || 'pouring' => 'rain',
    'lightning' || 'dark' => 'storm',
    'snowy' => 'overcast',
    _ => condition,
  };
}

String _outdoorOverrideDuration(double? expiresInMinutes) {
  if (expiresInMinutes == null) return 'forever';
  if (expiresInMinutes <= 0) return 'expires now';
  return _formatMinutes(expiresInMinutes.round());
}

List<String> _zhaNumericOptions({
  required double? min,
  required double? max,
  required double? step,
}) {
  if (min == null || max == null || max < min) return const [];
  final safeStep = (step == null || step <= 0) ? 1.0 : step;
  final values = <String>[];
  var current = min;
  var guard = 0;
  while (current <= max + 0.0001 && guard < 100) {
    values.add(_formatConfigNumber(current));
    current += safeStep;
    guard++;
  }
  return values;
}

List<int> _zhaTimeoutOptions(int? current) {
  final values = <int>{15, 30, 60, 120, 180, 300, 600};
  if (current != null) values.add(current);
  return values.toList()..sort();
}

int _roundToStep(num value, int step) {
  return (value / step).round() * step;
}

int _closestLumenStep(double factor) {
  return _closestStepIndex(
    factor,
    [for (final step in _lumenSteps) step.factor],
  );
}

int _closestSolarStep(double exposure) {
  return _closestStepIndex(
    exposure,
    [for (final step in _solarSteps) step.exposure],
  );
}

String _sectionTuneDisplay({
  required String dimension,
  required int step,
  required bool inherited,
}) {
  if (dimension == 'balance') {
    final index = step.clamp(0, _lumenSteps.length - 1).toInt();
    final lumen = _lumenSteps[index];
    final text = '${lumen.label} ${(lumen.factor * 100).round()}%';
    return inherited ? 'area $text' : text;
  }

  final index = step.clamp(0, _solarSteps.length - 1).toInt();
  final solar = _solarSteps[index];
  final text = '${solar.label} ${(solar.exposure * 100).round()}%';
  return inherited ? 'area $text' : text;
}

int _closestStepIndex(double value, List<double> values) {
  var bestIndex = 0;
  var bestDistance = double.infinity;
  for (var i = 0; i < values.length; i++) {
    final distance = (values[i] - value).abs();
    if (distance < bestDistance) {
      bestDistance = distance;
      bestIndex = i;
    }
  }
  return bestIndex;
}

List<_ExpertLightGroup> _groupLightsBySection(List<_ExpertLightRow> rows) {
  final groups = <String, List<_ExpertLightRow>>{};
  for (final row in rows) {
    groups.putIfAbsent(row.sectionName, () => []).add(row);
  }
  final names = groups.keys.toList()
    ..sort((a, b) {
      if (a == 'Main section') return -1;
      if (b == 'Main section') return 1;
      return a.compareTo(b);
    });
  return [
    for (final name in names)
      _ExpertLightGroup(
        name: name,
        rows: groups[name]!..sort((a, b) => a.name.compareTo(b.name)),
      ),
  ];
}

String _formatMinutes(int minutes) {
  final absolute = minutes.abs();
  if (absolute < 60) return '${minutes}m';
  final hours = absolute ~/ 60;
  final remainder = absolute % 60;
  final sign = minutes < 0 ? '-' : '';
  if (remainder == 0) return '$sign${hours}h';
  return '$sign${hours}h ${remainder}m';
}

String? _sectionAutoOffLabel(String? autoOffAt) {
  if (autoOffAt == null || autoOffAt == 'forever') return null;
  final parsed = DateTime.tryParse(autoOffAt);
  if (parsed == null) return 'set';
  final remaining = parsed.toLocal().difference(DateTime.now());
  if (remaining.inMinutes <= 0) return 'due';
  if (remaining.inDays >= 1) return '${remaining.inDays}d';
  if (remaining.inHours >= 1) return '${remaining.inHours}h';
  return '${remaining.inMinutes}m';
}

String _formatSignedMinutes(int minutes) {
  if (minutes == 0) return 'at trigger';
  final formatted = _formatMinutes(minutes.abs());
  return minutes > 0 ? '+$formatted' : '-$formatted';
}

String _shortServerTime(String value) {
  final parsed = DateTime.tryParse(value);
  if (parsed == null) return value;
  final local = parsed.toLocal();
  return _formatHour(local.hour + local.minute / 60);
}

String _historyTimeLabel(Object? rawTs) {
  final ts = _nullableDouble(rawTs);
  if (ts == null) return '';
  final time = DateTime.fromMillisecondsSinceEpoch(
    (ts * 1000).round(),
    isUtc: false,
  );
  final now = DateTime.now();
  final age = now.difference(time);
  if (age.inMinutes < 1) return 'now';
  if (age.inHours < 1) return '${age.inMinutes}m';
  if (age.inDays < 1) return '${age.inHours}h';
  if (age.inDays < 7) return '${age.inDays}d';
  return _formatHour(time.hour + time.minute / 60);
}

String _activityDateBucket(double ts) {
  final time = DateTime.fromMillisecondsSinceEpoch((ts * 1000).round());
  final now = DateTime.now();
  final today = DateTime(now.year, now.month, now.day);
  final day = DateTime(time.year, time.month, time.day);
  final days = today.difference(day).inDays;
  if (days <= 0) return 'Today';
  if (days == 1) return 'Yesterday';
  if (days < 7) return 'This week';
  return 'Older';
}

String _activityActionLabel(String action) {
  return switch (action) {
    'turn_on' => 'Turned on',
    'turn_off' => 'Turned off',
    'circadian_on' => 'Circadian on',
    'circadian_off' => 'Circadian off',
    'boost' || 'boost_on' => 'Boost',
    'boost_off' || 'boost_end' => 'Boost ended',
    'freeze' => 'Freeze',
    'unfreeze' => 'Unfreeze',
    'auto_off_set' => 'Auto off set',
    'auto_off_cleared' => 'Auto off cleared',
    'glo_up' => 'Glo up',
    'glo_down' => 'Glo down',
    'glo_reset' => 'Glo reset',
    'full_send' => 'Full send',
    'sun_override' => 'Sun override',
    'sync_controls' => 'Sync controls',
    'sync_lights' => 'Sync lights',
    _ => _titleCase(action),
  };
}

String _sourceKindLabel(String sourceKind) {
  return switch (sourceKind) {
    'app' => 'App',
    'auto_schedule' => 'Schedule',
    'contact' => 'Contact',
    'motion' => 'Motion',
    'service_call' => 'Service',
    'switch' => 'Switch',
    'system' => 'System',
    'timer' => 'Timer',
    _ => _titleCase(sourceKind),
  };
}

String _activitySubtitle(_ExpertActivityEntry entry, String? areaName) {
  final parts = <String>[
    if (areaName != null && areaName.isNotEmpty) areaName,
    _sourceKindLabel(entry.sourceKind),
    if (entry.sourceEntity != null) entry.sourceEntity!,
    if (entry.fromValue != null && entry.toValue != null)
      '${_compactNumber(entry.fromValue!)} -> ${_compactNumber(entry.toValue!)}',
    if (entry.brightness != null) '${entry.brightness}%',
    if (entry.kelvin != null) '${entry.kelvin} K',
    if (entry.durationMinutes != null) _formatMinutes(entry.durationMinutes!),
    if (entry.intensity != null) 'boost ${entry.intensity}%',
    if ((entry.count ?? 0) > 1) 'x${entry.count}',
    if (entry.eventId != null) '#${entry.eventId}',
  ];
  final vector = _stringValue(entry.payload?['vector']);
  if (vector != null) parts.add(_titleCase(vector));
  return parts.join(' · ');
}

String _momentActionLabel(String action) {
  return switch (action) {
    'off' => 'Off',
    'lights_on' || 'on' => 'On',
    'nitelite' => 'NiteLite',
    'britelite' => 'BriteLite',
    'wake_or_bed' => 'Wake / Bed',
    'circadian_off' => 'Circadian off',
    'leave_alone' => 'Leave alone',
    'reset' => 'Reset',
    _ => _titleCase(action),
  };
}

String _momentCategoryLabel(String category) {
  for (final option in _momentCategoryOptions) {
    if (option.$1 == category) return option.$2;
  }
  return _titleCase(category);
}

String _momentIconText(String icon) {
  final text = icon.trim();
  if (text.isEmpty) return '*';
  final entityMatch = RegExp(r'&#(\d+);').firstMatch(text);
  if (entityMatch != null) {
    final codePoint = int.tryParse(entityMatch.group(1)!);
    if (codePoint != null) return String.fromCharCode(codePoint);
  }
  if (text.startsWith('mdi:')) {
    final name = text.substring(4).replaceAll(RegExp(r'[-_]'), ' ').trim();
    if (name.isEmpty) return '*';
    return name.substring(0, math.min(2, name.length)).toUpperCase();
  }

  final cleaned = text
      .replaceAll('&amp;', '&')
      .replaceAll(RegExp(r'&#\d+;'), '')
      .replaceAll(RegExp(r'<[^>]*>'), '')
      .trim();
  if (cleaned.isEmpty) return '*';
  return String.fromCharCodes(cleaned.runes.take(2));
}

String _momentSubtitle(_ExpertMoment moment) {
  final timerMinutes = (moment.timerSeconds / 60).round();
  final parts = <String>[
    _momentActionLabel(moment.defaultAction),
    _titleCase(moment.category),
    if (timerMinutes > 0) _formatMinutes(timerMinutes),
    if (moment.exceptions.isNotEmpty)
      '${moment.exceptions.length} ${moment.exceptions.length == 1 ? 'exception' : 'exceptions'}',
    if (moment.usageCount > 0)
      '${moment.usageCount} ${moment.usageCount == 1 ? 'switch' : 'switches'}'
    else
      'unused',
  ];
  return parts.join(' · ');
}

Map<String, Object?> _momentExceptionsToServer(
  Map<String, _ExpertMomentException> exceptions,
) {
  return {
    for (final entry in exceptions.entries) entry.key: entry.value.toServer(),
  };
}

List<_ExpertMomentAssignment> _momentAssignmentsFor(
  String momentId,
  List<_ExpertControl> controls,
) {
  final actionRef = 'set_$momentId';
  final assignments = <_ExpertMomentAssignment>[];
  for (final control in controls) {
    if (!control.isSwitch) continue;
    for (final entry in control.magicButtons.entries) {
      if (entry.value == actionRef) {
        assignments.add(
          _ExpertMomentAssignment(control: control, event: entry.key),
        );
      }
    }
  }
  assignments.sort((a, b) {
    final controlCompare = a.control.name.toLowerCase().compareTo(
          b.control.name.toLowerCase(),
        );
    return controlCompare != 0
        ? controlCompare
        : _compareButtonEvents(a.event, b.event);
  });
  return assignments;
}

List<String> _momentButtonEventsFor(
  _ExpertControl control,
  _ExpertSwitchType? type,
) {
  final events = <String>{...control.magicButtons.keys};
  if (type != null) {
    events.addAll(type.defaultMapping.keys);
    if (events.isEmpty) {
      for (final button in type.buttons) {
        for (final actionType in type.actionTypes.take(4)) {
          events.add('${button}_$actionType');
        }
      }
    }
  }
  final list = events.toList()..sort(_compareButtonEvents);
  return list;
}

String _magicAssignmentLabel(String action, List<_ExpertMoment> moments) {
  if (action.startsWith('set_')) {
    final momentId = action.substring(4);
    for (final moment in moments) {
      if (moment.id == momentId) return moment.name;
    }
    return momentId;
  }
  return _activityActionLabel(action);
}

String _switchSubtitle(_ExpertSwitch control) {
  final parts = <String>[
    control.typeName,
    if (control.areaName != null) control.areaName!,
    '${control.scopes.length} ${control.scopes.length == 1 ? 'scope' : 'scopes'}',
    if (control.momentAssignmentCount > 0)
      '${control.momentAssignmentCount} moment ${control.momentAssignmentCount == 1 ? 'button' : 'buttons'}',
    if (control.inactiveUntil != null) 'paused ${control.inactiveUntil}',
  ];
  return parts.join(' · ');
}

String _controlSubtitle(_ExpertControl control) {
  final parts = <String>[
    control.categoryLabel,
    if (control.typeName != control.categoryLabel) control.typeName,
    if (control.areaName != null) control.areaName!,
    _controlStatusLabel(control.status),
    if (control.batteryLevel != null) '${control.batteryLevel}% battery',
    if (control.illuminance != null) '${control.illuminance!.round()} lx',
    if (control.momentAssignmentCount > 0)
      '${control.momentAssignmentCount} moment ${control.momentAssignmentCount == 1 ? 'button' : 'buttons'}',
    if (control.scopes.isNotEmpty)
      '${control.scopes.length} ${control.scopes.length == 1 ? 'scope' : 'scopes'}',
    if (_controlLastActionLabel(control) != null)
      _controlLastActionLabel(control)!,
  ];
  return parts.join(' · ');
}

bool _controlTargetsArea(
  _ExpertControl control,
  String areaId, {
  Set<String> sectionIds = const {},
}) {
  if (control.areaId == areaId) return true;
  return _controlDirectlyScopesArea(
    control,
    areaId,
    sectionIds: sectionIds,
  );
}

bool _controlDirectlyScopesArea(
  _ExpertControl control,
  String areaId, {
  Set<String> sectionIds = const {},
}) {
  for (final scope in control.scopes) {
    if (scope.areaIds.contains(areaId)) return true;
    if (sectionIds.isNotEmpty &&
        scope.sectionIds.any((sectionId) => sectionIds.contains(sectionId))) {
      return true;
    }
  }
  return false;
}

String _controlAreaReachLabel(
  _ExpertControl control,
  String areaId, {
  Set<String> sectionIds = const {},
}) {
  final locatedHere = control.areaId == areaId;
  final scopedHere = _controlDirectlyScopesArea(
    control,
    areaId,
    sectionIds: sectionIds,
  );
  if (locatedHere && scopedHere) return 'Located here · controls this area';
  if (locatedHere) return 'Located here';
  if (scopedHere && _controlScopesOnlySections(control, areaId, sectionIds)) {
    return 'Controls a section';
  }
  if (scopedHere) return 'Controls this area';
  return 'Related control';
}

bool _controlScopesOnlySections(
  _ExpertControl control,
  String areaId,
  Set<String> sectionIds,
) {
  if (sectionIds.isEmpty) return false;
  for (final scope in control.scopes) {
    if (scope.areaIds.contains(areaId)) return false;
  }
  return control.scopes.any(
    (scope) =>
        scope.sectionIds.any((sectionId) => sectionIds.contains(sectionId)),
  );
}

int _compareAreaControls(
  String areaId,
  _ExpertControl a,
  _ExpertControl b, {
  Set<String> sectionIds = const {},
}) {
  final aScopeRank =
      _controlDirectlyScopesArea(a, areaId, sectionIds: sectionIds) ? 0 : 1;
  final bScopeRank =
      _controlDirectlyScopesArea(b, areaId, sectionIds: sectionIds) ? 0 : 1;
  if (aScopeRank != bScopeRank) return aScopeRank.compareTo(bScopeRank);

  final aStatusRank = _areaControlStatusRank(a);
  final bStatusRank = _areaControlStatusRank(b);
  if (aStatusRank != bStatusRank) return aStatusRank.compareTo(bStatusRank);

  final aCategory = a.categoryLabel.toLowerCase();
  final bCategory = b.categoryLabel.toLowerCase();
  final categoryCompare = aCategory.compareTo(bCategory);
  if (categoryCompare != 0) return categoryCompare;

  return a.name.toLowerCase().compareTo(b.name.toLowerCase());
}

int _areaControlStatusRank(_ExpertControl control) {
  if (control.stale || control.status == 'stale') return 4;
  if (_controlNeedsIntegration(control.status)) return 3;
  if (control.status == 'not_configured') return 2;
  if (control.inactive || control.status == 'inactive') return 1;
  return 0;
}

String _controlCategoryLabel(String category) {
  return switch (category) {
    'switch' => 'Switch',
    'motion_sensor' => 'Motion',
    'contact_sensor' => 'Contact',
    'camera' => 'Camera',
    'unknown' => 'Unknown',
    _ => _titleCase(category.replaceAll('_', ' ')),
  };
}

IconData _controlCategoryIcon(String category) {
  return switch (category) {
    'motion_sensor' => Icons.sensors_rounded,
    'contact_sensor' => Icons.sensor_door_rounded,
    'camera' => Icons.photo_camera_rounded,
    'switch' => Icons.settings_remote_rounded,
    _ => Icons.tune_rounded,
  };
}

String _controlSourceDeviceMeta(_ExpertControlSourceDevice device) {
  final parts = [
    if (device.manufacturer != null) device.manufacturer!,
    if (device.model != null) device.model!,
    if (device.areaName != null) device.areaName!,
    '${device.binarySensors.length} ${device.binarySensors.length == 1 ? 'sensor' : 'sensors'}',
  ];
  return parts.isEmpty ? device.deviceId : parts.join(' · ');
}

int _controlSourceSensorRank(_ExpertControlSourceSensor sensor) {
  final searchable =
      '${sensor.entityId} ${sensor.name} ${sensor.deviceClass}'.toLowerCase();
  const preferred = [
    'motion',
    'person',
    'human',
    'presence',
    'occupancy',
    'ringing',
    'package',
    'vehicle',
    'pet',
    'animal',
  ];
  final index = preferred.indexWhere((keyword) => searchable.contains(keyword));
  return index == -1 ? preferred.length : index;
}

List<String> _controlRefreshKeys(_ExpertControl control) {
  final keys = <String>[];
  final primary = {'motion_sensor', 'contact_sensor'}.contains(control.category)
      ? control.deviceId
      : control.id;
  for (final value in [primary, control.id, control.deviceId]) {
    if (value != null && value.isNotEmpty && !keys.contains(value)) {
      keys.add(value);
    }
  }
  return keys;
}

Map<dynamic, dynamic>? _firstMapForKeys(
  Map<String, Map<dynamic, dynamic>> values,
  List<String> keys,
) {
  for (final key in keys) {
    final value = values[key];
    if (value != null) return value;
  }
  return null;
}

_ExpertControlPauseState? _firstPauseForKeys(
  Map<String, _ExpertControlPauseState> values,
  List<String> keys,
) {
  for (final key in keys) {
    final value = values[key];
    if (value != null) return value;
  }
  return null;
}

bool _mapStringEquals(Map<dynamic, dynamic> a, Map<dynamic, dynamic>? b) {
  if (b == null || a.length != b.length) return false;
  for (final entry in a.entries) {
    if (entry.value.toString() != b[entry.key].toString()) return false;
  }
  return true;
}

String _controlStatusLabel(String status) {
  return switch (status) {
    'active' => 'Active',
    'inactive' => 'Paused',
    'not_configured' => 'Setup',
    'needs_integration' => 'Needs integration',
    'unsupported' => 'Needs integration',
    'stale' => 'Stale',
    _ => _titleCase(status.replaceAll('_', ' ')),
  };
}

bool _controlNeedsIntegration(String status) =>
    status == 'needs_integration' || status == 'unsupported';

String _controlStatusWithPause(String status, bool inactive) {
  if (_controlNeedsIntegration(status) ||
      status == 'stale' ||
      status == 'not_configured') {
    return status;
  }
  if (inactive) return 'inactive';
  return status == 'inactive' ? 'active' : status;
}

int _controlStatusRank(String status) {
  return switch (status) {
    'active' => 0,
    'inactive' => 1,
    'not_configured' => 2,
    'needs_integration' => 3,
    'unsupported' => 3,
    'stale' => 4,
    _ => 9,
  };
}

String _entityName(String entityId) {
  final short = entityId.contains('.') ? entityId.split('.').last : entityId;
  return _titleCase(short.replaceAll('_', ' '));
}

List<_TriggerEntityGroup> _triggerGroupsForSensors(
  List<_ExpertTriggerEntity> sensors,
) {
  const specs = [
    (
      id: 'any_motion',
      label: 'Any motion',
      isAny: true,
      own: ['_motion_detected', '_motion', '_presence', '_occupancy'],
      extra: [
        '_person_detected',
        '_person',
        '_human_detected',
        '_human',
        '_pet_detected',
        '_animal_detected',
        '_animal',
        '_vehicle_detected',
        '_vehicle',
      ],
    ),
    (
      id: 'person',
      label: 'Person',
      isAny: false,
      own: ['_person_detected', '_person', '_human_detected', '_human'],
      extra: <String>[],
    ),
    (
      id: 'pet',
      label: 'Pet',
      isAny: false,
      own: ['_pet_detected', '_animal_detected', '_animal'],
      extra: <String>[],
    ),
    (
      id: 'vehicle',
      label: 'Vehicle',
      isAny: false,
      own: ['_vehicle_detected', '_vehicle'],
      extra: <String>[],
    ),
    (
      id: 'package',
      label: 'Package',
      isAny: false,
      own: [
        '_package_delivered',
        '_package_stranded',
        '_package_taken',
        '_package',
      ],
      extra: <String>[],
    ),
    (
      id: 'doorbell',
      label: 'Doorbell',
      isAny: false,
      own: ['_ringing', '_ding', '_doorbell'],
      extra: <String>[],
    ),
  ];

  final groups = <_TriggerEntityGroup>[];
  for (final spec in specs) {
    final own = [
      for (final sensor in sensors)
        if (spec.own.any(sensor.entityId.contains)) sensor.entityId,
    ];
    final allKeywords = [...spec.own, ...spec.extra];
    final all = [
      for (final sensor in sensors)
        if (allKeywords.any(sensor.entityId.contains)) sensor.entityId,
    ];
    if (all.isEmpty) continue;
    groups.add(
      _TriggerEntityGroup(
        id: spec.id,
        label: spec.label,
        isAny: spec.isAny,
        ownEntityIds: own,
        allEntityIds: all,
      ),
    );
  }
  return groups;
}

bool _triggerGroupSelected(
  List<String> selected,
  _TriggerEntityGroup group,
) {
  if (group.isAny && selected.isEmpty) return true;
  return group.ownEntityIds.any(selected.contains);
}

List<String> _toggleTriggerGroup(
  List<String> selected,
  _TriggerEntityGroup group,
) {
  if (group.isAny) return const [];
  final next = selected.isEmpty ? <String>{} : selected.toSet();
  final isSelected = group.ownEntityIds.any(next.contains);
  if (isSelected) {
    next.removeAll(group.allEntityIds);
  } else {
    next.addAll(group.allEntityIds);
  }
  final values = next.toList()..sort();
  return values;
}

List<String> _toggleTriggerEntity(List<String> selected, String entityId) {
  final next = selected.isEmpty ? <String>{} : selected.toSet();
  if (!next.add(entityId)) next.remove(entityId);
  final values = next.toList()..sort();
  return values;
}

String? _controlLastActionLabel(_ExpertControl control) {
  final action = control.lastAction;
  if (action == null) return null;
  final timestamp = _stringValue(action['timestamp']);
  final age = timestamp == null ? null : _shortRelativeIso(timestamp);
  final label = _stringValue(action['action']) ??
      [
        _stringValue(action['button']),
        _stringValue(action['press']),
      ].whereType<String>().join(' ');
  if (label.isEmpty) {
    return age == null ? null : 'Used $age';
  }
  return age == null ? label : '$label $age';
}

String? _shortRelativeIso(String value) {
  final parsed = DateTime.tryParse(value);
  if (parsed == null) return null;
  final delta = DateTime.now().difference(parsed.toLocal());
  if (delta.inMinutes < 1) return 'now';
  if (delta.inMinutes < 60) return '${delta.inMinutes}m ago';
  if (delta.inHours < 24) return '${delta.inHours}h ago';
  return '${delta.inDays}d ago';
}

String? _pauseUntilIso(int minutes) {
  if (minutes <= 0) return 'forever';
  return DateTime.now()
      .toUtc()
      .add(Duration(minutes: minutes))
      .toIso8601String();
}

int? _secondsUntil(String? inactiveUntil) {
  if (inactiveUntil == null || inactiveUntil == 'forever') return null;
  final parsed = DateTime.tryParse(inactiveUntil);
  if (parsed == null) return null;
  return math.max(0, parsed.toLocal().difference(DateTime.now()).inSeconds);
}

String _pausePillLabel(String? inactiveUntil) {
  if (inactiveUntil == null || inactiveUntil == 'forever') return 'PAUSED';
  final parsed = DateTime.tryParse(inactiveUntil);
  if (parsed == null) return 'PAUSED';
  final remaining = parsed.toLocal().difference(DateTime.now());
  if (remaining.inMinutes <= 0) return 'PAUSED';
  if (remaining.inHours >= 1) return 'PAUSED ${remaining.inHours}H';
  return 'PAUSED ${remaining.inMinutes}M';
}

String _pauseUntilText(String? inactiveUntil) {
  if (inactiveUntil == null || inactiveUntil == 'forever') return 'forever';
  final parsed = DateTime.tryParse(inactiveUntil);
  if (parsed == null) return '';
  final remaining = parsed.toLocal().difference(DateTime.now());
  if (remaining.inMinutes <= 0) return 'expired';
  if (remaining.inHours >= 24) return '${remaining.inDays}d';
  if (remaining.inHours >= 1) return '${remaining.inHours}h';
  return '${remaining.inMinutes}m';
}

String _sensorModeLabel(String value) {
  return switch (value) {
    'on_only' => 'Turn on',
    'on_off' => 'Turn on with timer',
    'alert' => 'Alert',
    'disabled' => 'Disabled',
    _ => _titleCase(value.replaceAll('_', ' ')),
  };
}

String _sensorScheduleLabel(String value) {
  return switch (value) {
    'sunset_to_sunrise' => 'Sunset to sunrise',
    'wake_to_bed' => 'Wake to bed',
    'always' => 'Always',
    _ => _titleCase(value.replaceAll('_', ' ')),
  };
}

String _alertIntensityLabel(String value) {
  return switch (value) {
    'low' => 'Low',
    'med' => 'Med',
    'high' => 'High',
    _ => _titleCase(value),
  };
}

String _controlScopeTitle(
  int index,
  _ExpertSwitchScope scope,
  String Function(String areaId) areaName,
  String Function(String sectionId) sectionName,
) {
  final targets = [
    for (final areaId in scope.areaIds) areaName(areaId),
    for (final sectionId in scope.sectionIds) sectionName(sectionId),
  ];
  final targetLabel = targets.isEmpty
      ? 'No targets'
      : targets.length <= 2
          ? targets.join(', ')
          : '${targets.take(2).join(', ')} +${targets.length - 2}';
  return 'Reach ${index + 1} · $targetLabel';
}

List<_ExpertReachSection> _reachSectionsForAreas(List<_ExpertArea> areas) {
  final seen = <String>{};
  final sections = <_ExpertReachSection>[];
  for (final area in areas) {
    for (final section in area.sections) {
      if (!seen.add(section.id)) continue;
      sections.add(
        _ExpertReachSection(
          id: section.id,
          name: section.name,
          areaId: area.id,
          areaName: area.name,
        ),
      );
    }
  }
  return sections;
}

String _sectionReachLabel(List<_ExpertArea> areas, String sectionId) {
  for (final section in _reachSectionsForAreas(areas)) {
    if (section.id == sectionId) return section.label;
  }
  return sectionId;
}

String? _scopeFeedbackArea(_ExpertSwitchScope scope) {
  final explicit = scope.feedbackArea;
  if (explicit != null && scope.areaIds.contains(explicit)) return explicit;
  if (scope.areaIds.isEmpty) return null;
  return scope.areaIds.first;
}

_ExpertSwitchScope _scopeWithAreaIds(
  _ExpertSwitchScope scope,
  Iterable<String> areaIds,
) {
  final nextAreaIds = _uniqueOrdered(areaIds);
  final feedbackArea = scope.feedbackArea;
  return scope.copyWith(
    areaIds: nextAreaIds,
    feedbackArea: feedbackArea != null && nextAreaIds.contains(feedbackArea)
        ? feedbackArea
        : null,
  );
}

_ExpertSwitchScope _scopeWithSectionIds(
  _ExpertSwitchScope scope,
  Iterable<String> sectionIds,
) {
  return scope.copyWith(sectionIds: _uniqueOrdered(sectionIds));
}

_ExpertSwitchScope _scopeWithFeedbackArea(
  _ExpertSwitchScope scope,
  String areaId,
) {
  if (!scope.areaIds.contains(areaId)) return scope;
  return scope.copyWith(
    areaIds: [
      areaId,
      for (final current in scope.areaIds)
        if (current != areaId) current,
    ],
    feedbackArea: areaId,
  );
}

List<String> _uniqueOrdered(Iterable<String> values) {
  final seen = <String>{};
  return [
    for (final value in values)
      if (value.isNotEmpty && seen.add(value)) value,
  ];
}

int _compareButtonEvents(String a, String b) {
  final aParts = a.split('_');
  final bParts = b.split('_');
  const buttonOrder = {
    'on': 0,
    'up': 1,
    'down': 2,
    'off': 3,
    'toggle': 4,
    'dial': 5,
    'arrow': 6,
  };
  const actionOrder = {
    'press': 0,
    'short': 1,
    'hold': 2,
    'long': 3,
    'double': 4,
    'triple': 5,
    'quadruple': 6,
    'quintuple': 7,
    'rotate': 8,
  };
  final buttonCompare = (buttonOrder[aParts.first] ?? 99)
      .compareTo(buttonOrder[bParts.first] ?? 99);
  if (buttonCompare != 0) return buttonCompare;
  final actionCompare = (actionOrder[aParts.length > 1 ? aParts[1] : ''] ?? 99)
      .compareTo(actionOrder[bParts.length > 1 ? bParts[1] : ''] ?? 99);
  return actionCompare != 0 ? actionCompare : a.compareTo(b);
}

String _buttonEventLabel(String event) {
  const buttonNames = {
    'on': 'Power',
    'up': 'Up',
    'down': 'Down',
    'off': 'Hue',
    'toggle': 'Toggle',
    'dial': 'Dial',
    'arrow_left': 'Left',
    'arrow_right': 'Right',
  };
  const actionNames = {
    'hold': 'Long press',
    'short_release': 'Press',
    'press': 'Press',
    'long_release': 'Release',
    'double_press': '2x press',
    'triple_press': '3x press',
    'quadruple_press': '4x press',
    'quintuple_press': '5x press',
    'rotate': 'Rotate',
  };
  const suffixes = [
    'quintuple_press',
    'quadruple_press',
    'triple_press',
    'double_press',
    'short_release',
    'long_release',
    'press',
    'hold',
    'rotate',
  ];
  for (final suffix in suffixes) {
    final marker = '_$suffix';
    if (!event.endsWith(marker)) continue;
    final buttonKey = event.substring(0, event.length - marker.length);
    final button = buttonNames[buttonKey] ?? _titleCase(buttonKey);
    final action = actionNames[suffix] ?? _titleCase(suffix);
    return '$button $action';
  }
  return _titleCase(event);
}

String _magicActionLabel(Object? rawAction, List<_ExpertMoment> moments) {
  final rawMap = _asMapOrNull(rawAction);
  final action = rawMap == null
      ? _stringValue(rawAction)
      : _stringValue(rawMap['action'] ?? rawMap['value']);
  if (action == null) return 'Default: none';
  if (action == 'magic') return 'Default: magic button';
  if (action.startsWith('set_')) {
    final momentId = action.substring(4);
    for (final moment in moments) {
      if (moment.id == momentId) return 'Default: ${moment.name}';
    }
  }
  return 'Default: ${_activityActionLabel(action)}';
}

String? _mappingMainAction(Object? value) {
  final body = _asMapOrNull(value);
  if (body != null) return _stringValue(body['action']);
  return _stringValue(value);
}

String? _mappingWhenOff(Object? value) {
  final body = _asMapOrNull(value);
  return body == null ? null : _stringValue(body['when_off']);
}

Object? _mappingToServer(Object? value) {
  final body = _asMapOrNull(value);
  if (body == null) return _mappingMainAction(value);
  return {
    'action': _mappingMainAction(value),
    'when_off': _mappingWhenOff(value),
  };
}

bool _mappingEquals(Object? a, Object? b) {
  return _mappingMainAction(a) == _mappingMainAction(b) &&
      _mappingWhenOff(a) == _mappingWhenOff(b);
}

bool _isAdjustmentAction(String? action) {
  return const {
    'step_up',
    'step_down',
    'step_up_2',
    'step_down_2',
    'step_up_3',
    'step_down_3',
    'bright_up',
    'bright_down',
    'bright_up_2',
    'bright_down_2',
    'bright_up_3',
    'bright_down_3',
    'bright_up_4',
    'bright_down_4',
    'bright_up_5',
    'bright_down_5',
    'color_up',
    'color_down',
  }.contains(action);
}

String _buttonName(String button) {
  return switch (button) {
    'on' => 'Power',
    'off' => 'Hue Button',
    'dial' => 'Dial Button',
    'up' => 'Up',
    'down' => 'Down',
    'toggle' => 'Toggle',
    'brightness_up' => 'Bright Up',
    'brightness_down' => 'Bright Down',
    'arrow_left' => 'Left',
    'arrow_right' => 'Right',
    _ => _titleCase(button),
  };
}

String _buttonBadge(String button) {
  return switch (button) {
    'on' => 'I',
    'up' => '+',
    'down' => '-',
    'off' => 'O',
    'dial' => 'D',
    'toggle' => 'T',
    'brightness_up' => 'B+',
    'brightness_down' => 'B-',
    'arrow_left' => '<',
    'arrow_right' => '>',
    _ => button.substring(0, math.min(2, button.length)).toUpperCase(),
  };
}

String _actionTypeLabel(String actionType) {
  return switch (actionType) {
    'short_release' || 'press' => 'Short press',
    'double_press' => '2x press',
    'triple_press' => '3x press',
    'quadruple_press' => '4x press',
    'quintuple_press' => '5x press',
    'hold' => 'Long press',
    'rotate' => 'Rotate',
    _ => _titleCase(actionType),
  };
}

String _compactNumber(double value) {
  if ((value - value.round()).abs() < 0.05) return '${value.round()}';
  return value.toStringAsFixed(1);
}

String _compactCoordinate(double value) {
  final fixed = value.toStringAsFixed(2);
  if (fixed == '-0.00') return '0.00';
  return fixed;
}

String _titleCase(String value) {
  return value
      .replaceAll('_', ' ')
      .split(' ')
      .where((part) => part.isNotEmpty)
      .map((part) => part[0].toUpperCase() + part.substring(1))
      .join(' ');
}

String _identityString(String value) => value;

List<String> _stringOptionsWithValue(List<String> options, String value) {
  final items = <String>[
    for (final option in options)
      if (option.trim().isNotEmpty) option,
  ];
  if (!items.contains(value)) items.insert(0, value);
  return items;
}

String _patternLabel(String pattern) {
  return switch (pattern) {
    'young' => 'Young child',
    'adult' => 'Typical adult',
    'nightowl' => 'Night owl',
    'duskbat' => 'Dusk bat',
    'shiftearly' => 'Early night shift',
    'shiftlate' => 'Late night shift',
    'early' => 'Early',
    'standard' => 'Standard',
    'late' => 'Late',
    'shift' => 'Shift',
    _ => _titleCase(pattern),
  };
}

double _roundQuarter(double value) {
  return (_normalizeHour(value) * 4).round() / 4;
}

double _roundHundredth(double value) {
  return (value * 100).round() / 100;
}

String _normalizeBaseUrl(String baseUrl) {
  final trimmed = baseUrl.trim();
  return trimmed.endsWith('/') ? trimmed : '$trimmed/';
}

String _trimTrailingSlash(String baseUrl) {
  var trimmed = baseUrl.trim();
  while (trimmed.endsWith('/')) {
    trimmed = trimmed.substring(0, trimmed.length - 1);
  }
  return trimmed;
}

String _isoDate(DateTime date) {
  final local = date.toLocal();
  return '${local.year.toString().padLeft(4, '0')}-'
      '${local.month.toString().padLeft(2, '0')}-'
      '${local.day.toString().padLeft(2, '0')}';
}

String _scheduleOverrideUntilDate(String through) {
  final now = DateTime.now();
  final today = DateTime(now.year, now.month, now.day);
  return switch (through) {
    'today' => _isoDate(today),
    'forever' => '2099-12-31',
    _ => _isoDate(today.add(const Duration(days: 1))),
  };
}

Map<String, String>? _bearerHeaders(String? authToken) {
  final token = authToken?.trim();
  if (token == null || token.isEmpty) return null;
  return {'Authorization': 'Bearer $token'};
}

double _dayShape(double hour, double peakHour) {
  final diff = _circularDistance(hour, peakHour);
  final radians = (diff / 12) * math.pi;
  return ((math.cos(radians) + 1) / 2).clamp(0.0, 1.0);
}

double _circularDistance(double a, double b) {
  final raw = (a - b).abs() % 24;
  return math.min(raw, 24 - raw);
}

bool _isInSleepWindow(double hour, double bed, double wake) {
  final bedHour = _normalizeHour(bed);
  final wakeHour = _normalizeHour(wake);
  if (bedHour < wakeHour) {
    return hour >= bedHour && hour < wakeHour;
  }
  return hour >= bedHour || hour < wakeHour;
}

double _normalizeHour(double hour) {
  final value = hour % 24;
  return value < 0 ? value + 24 : value;
}

double _patternWake(String pattern) {
  return switch (pattern) {
    'young' => 6.0,
    'adult' => 7.0,
    'nightowl' => 10.0,
    'duskbat' => 14.0,
    'shiftearly' => 18.0,
    'shiftlate' => 22.0,
    'early' => 5.75,
    'late' => 8.0,
    'shift' => 11.0,
    _ => 6.5,
  };
}

double _patternBed(String pattern) {
  return switch (pattern) {
    'young' => 18.0,
    'adult' => 21.0,
    'nightowl' => 2.0,
    'duskbat' => 6.0,
    'shiftearly' => 10.0,
    'shiftlate' => 14.0,
    'early' => 21.75,
    'late' => 0.5,
    'shift' => 2.5,
    _ => 22.5,
  };
}

double _phaseBalanceFromAscend(double ascendStart, double wakeHour) {
  final defaultAscend = _normalizeHour(wakeHour - 3.5);
  var delta = _normalizeHour(ascendStart) - defaultAscend;
  if (delta > 12) delta -= 24;
  if (delta < -12) delta += 24;
  return delta.clamp(-1, 1).toDouble();
}

String _formatHour(double hour) {
  final normalized = _normalizeHour(hour);
  final totalMinutes = (normalized * 60).round() % (24 * 60);
  final h24 = totalMinutes ~/ 60;
  final minutes = totalMinutes % 60;
  final suffix = h24 >= 12 ? 'PM' : 'AM';
  final h12 = h24 % 12 == 0 ? 12 : h24 % 12;
  return '$h12:${minutes.toString().padLeft(2, '0')} $suffix';
}

String _axisHour(int hour) {
  if (hour == 0 || hour == 24) return '12a';
  if (hour == 12) return '12p';
  return hour < 12 ? '${hour}a' : '${hour - 12}p';
}

String _shortWeekday(int day) {
  const labels = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
  return labels[day.clamp(0, 6)];
}

String _areaCountLabel(int count) {
  return count == 1 ? '1 area' : '$count areas';
}

String _signedPercent(double value) {
  final percent = (value * 100).round();
  if (percent == 0) return 'center';
  return percent > 0 ? '+$percent%' : '$percent%';
}

String _signedValue(double value) {
  if (value.abs() < 0.05) return '0';
  final formatted = _compactNumber(value.abs());
  return value > 0 ? '+$formatted' : '-$formatted';
}

double _decimalNow(TimeOfDay time) {
  return time.hour + time.minute / 60;
}

TimeOfDay _timeOfDayFromHour(double hour) {
  final normalized = _normalizeHour(hour);
  final totalMinutes = (normalized * 60).round() % (24 * 60);
  return TimeOfDay(
    hour: totalMinutes ~/ 60,
    minute: totalMinutes % 60,
  );
}

Color _cctToColor(int kelvin) {
  final temp = (kelvin / 100).clamp(10, 400).toDouble();
  double red;
  double green;
  double blue;

  if (temp <= 66) {
    red = 255;
    green = 99.4708025861 * math.log(temp) - 161.1195681661;
    blue =
        temp <= 19 ? 0 : 138.5177312231 * math.log(temp - 10) - 305.0447927307;
  } else {
    red = 329.698727446 * math.pow(temp - 60, -0.1332047592).toDouble();
    green = 288.1221695283 * math.pow(temp - 60, -0.0755148492).toDouble();
    blue = 255;
  }

  return Color.fromARGB(
    255,
    red.round().clamp(0, 255),
    green.round().clamp(0, 255),
    blue.round().clamp(0, 255),
  );
}

Color _sliderPreviewColor(_SliderPreviewPoint point) {
  final brightness = point.brightness.clamp(0, 100).toDouble() / 100;
  final tint = _cctToColor(point.kelvin);
  final amount = (0.18 + brightness * 0.82).clamp(0.0, 1.0).toDouble();
  return Color.lerp(
    _panelColor,
    tint,
    amount,
  )!
      .withValues(alpha: 0.95);
}

Color _readableOn(Color color) {
  return color.computeLuminance() > 0.55
      ? const Color(0xFF14110E)
      : CelestialColors.textPrimary;
}
