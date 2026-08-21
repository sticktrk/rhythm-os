import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart' hide Home, Hub, HubType;
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../../models/plan_tier.dart';
import '../../providers/home_provider.dart';
import '../../providers/server_sync_provider.dart';
import '../../providers/subscription_provider.dart';
import '../../widgets/info_tooltip.dart';
import '../../widgets/mode_summary_chip.dart';
import '../../widgets/plan_tier_modal.dart';
import '../../widgets/rhythm_clock/rhythm_clock_visuals.dart';
import '../../widgets/rhythm_clock/rhythm_schedule_clock.dart';
import '../../widgets/solar_clock/solar_clock_exports.dart';
import '../../widgets/solar_orbit.dart';

/// Single accent for page chrome (hero card, accordions, switches, save
/// bar, reset pill, button section). Stable identity that doesn't shift
/// with `_selectedMode`. Mode color (Day/Sleep) still lives in the orbital
/// ring + orbs + anchor markers + Day/Sleep summary chips.
const Color _chromeAccent = Colors.white;

// Surface tier above `CelestialColors.backgroundCard`. Used for the inner
// trigger-source cards so they read as elevated above the hero without
// resorting to stacked white-alpha washes.
const Color _surfaceElevated = Color(0xFF1F2630);
// One step below the hero card — used for the disabled/off state of an
// inner card so it recedes instead of glowing.
const Color _surfaceRecessed = Color(0xFF11161E);
// Deterministic hairline stroke shared by all chrome surfaces. Replaces
// the previous `Colors.white.withValues(alpha: …)` borders that hazed
// when stacked.
const Color _hairlineStrong = Color(0xFF2C3441);
const Color _hairlineSoft = Color(0xFF1E2530);

/// Which single automation this editor instance is the detail screen for.
/// Each automation in the Automations list pushes its own detail; this picks
/// which section (and which controls) render.
enum AutomationDetail { automatic, button }

class DefaultTransitionEditorScreen extends StatefulWidget {
  final Map<RhythmMode, Color> profileColors;

  /// Which automation's detail to render — the schedule (orbital clock) or the
  /// physical button binding. The transition-duration control is shown in both.
  final AutomationDetail detail;

  const DefaultTransitionEditorScreen({
    super.key,
    required this.profileColors,
    required this.detail,
  });

  @override
  State<DefaultTransitionEditorScreen> createState() =>
      _DefaultTransitionEditorScreenState();
}

class _DefaultTransitionEditorScreenState
    extends State<DefaultTransitionEditorScreen> with TickerProviderStateMixin {
  // Piecewise duration scale. Below the bend, the slider steps 1s at a time
  // so short transitions (think a quick room cue) are tunable; above the bend
  // it falls back to 10s steps so the long end stays reachable without 270
  // ticks across the track.
  static const double _minDurationSeconds = 1.0;
  static const double _bendDurationSeconds = 30.0;
  static const double _maxDurationSeconds = 300.0;
  static const double _fineDurationStepSeconds = 1.0;
  static const double _coarseDurationStepSeconds = 10.0;

  late Map<RhythmMode, RhythmModeTransitionConfig> _transitionConfigs;
  late Map<RhythmMode, RhythmModeTransitionConfig> _savedTransitionConfigs;
  // Trigger sources are independent toggles — either can be on without the
  // other, and both can be on at the same time. When both are off, mode
  // changes only happen by manual app interaction.
  bool _timeEnabled = true;
  bool _savedTimeEnabled = true;
  bool _buttonEnabled = false;
  // A single physical button acts as a global toggle between Day and Sleep,
  // shared by both directions when the Button source is enabled. Button-side
  // state is intentionally not tracked by the save/revert cluster — that
  // cluster lives inside the Time section and only governs Time edits. The
  // Button section commits changes inline via its own Listen → Confirm flow.
  String? _toggleButtonDeviceId;
  String _lastButtonBindingSignature = 'none';
  String _lastButtonDevicesSignature = '';
  // Listen-flow state. The user taps Listen, the server forwards the next
  // press, and we surface it for confirmation. `_pendingButton` is the
  // candidate from the most recent press — not committed until confirm.
  _ButtonBindState _bindState = _ButtonBindState.idle;
  _MockButtonDevice? _pendingButton;
  Timer? _simulatedDetectionTimer;
  StreamSubscription<RhythmInputEvent>? _inputEventSub;
  late final AnimationController _listenPulseController;
  late RhythmMode _selectedMode;
  final Map<RhythmMode, double> _handleHours = {};
  final Map<RhythmMode, ModeCurveVisual> _curveVisuals = {};
  late final ServerSyncProvider _serverSync;
  late final HomeProvider _homeProvider;
  String? _profileVisualSignature;
  SolarClockData? _solarClockData;
  String? _solarLocationKey;
  // Mirror of the clock widget's live drag state (via onDragPreview) so the
  // Day/Sleep summary chips can echo the preview time + anchor label.
  RhythmMode? _dragMode;
  double? _dragPreviewHour;
  TriggerAnchor? _proximateAnchor;
  double _anchorProximity = 0.0;
  bool _isSaving = false;
  // Tracks the last `_serverSync.synced` value so the save pill can be
  // rebuilt the moment the connection state flips.
  bool _lastSynced = false;

  SunTimesDto? get _sunTimes => _solarClockData?.sunTimes;
  TwilightTimesDto? get _twilightTimes => _solarClockData?.twilightTimes;
  RhythmModeTransitionConfig get _config => _transitionConfigs[_selectedMode]!;

  @override
  void initState() {
    super.initState();
    _serverSync = context.read<ServerSyncProvider>();
    _transitionConfigs = _initialTransitionConfigs();
    _timeEnabled = _timeEnabledFor(_transitionConfigs.values);
    _savedTransitionConfigs = Map.of(_transitionConfigs);
    _savedTimeEnabled = _timeEnabled;
    _selectedMode = RhythmMode.day;
    _listenPulseController = AnimationController(
      duration: const Duration(milliseconds: 1800),
      vsync: this,
    );
    _loadSolarTimes();
    _homeProvider = context.read<HomeProvider>();
    _homeProvider.addListener(_handleHomeChanged);
    _serverSync.addListener(_handleServerSyncChanged);
    _inputEventSub = _serverSync.inputEvents.listen(_handleInputEvent);
    _lastSynced = _serverSync.synced;
    _syncButtonBindingFromServer();
    _lastButtonBindingSignature = _serverSync.daySleepToggleBindingSignature;
    _lastButtonDevicesSignature = _buttonDevicesSignature();
  }

  @override
  void dispose() {
    _serverSync.removeListener(_handleServerSyncChanged);
    _homeProvider.removeListener(_handleHomeChanged);
    _listenPulseController.dispose();
    _simulatedDetectionTimer?.cancel();
    _inputEventSub?.cancel();
    super.dispose();
  }

  @override
  void didUpdateWidget(covariant DefaultTransitionEditorScreen oldWidget) {
    super.didUpdateWidget(oldWidget);
    _refreshProfileVisualsIfNeeded();
  }

  void _handleServerSyncChanged() {
    if (!mounted) return;

    final nextTransitionConfigs = _initialTransitionConfigs();
    final nextTimeEnabled = _timeEnabledFor(nextTransitionConfigs.values);
    final shouldAdoptTransitions = !_hasUnsavedChanges &&
        (!_transitionConfigMapsEqual(
              _transitionConfigs,
              nextTransitionConfigs,
            ) ||
            _timeEnabled != nextTimeEnabled);
    final shouldRefreshVisuals =
        _currentProfileVisualSignature() != _profileVisualSignature;
    final nextSynced = _serverSync.synced;
    final shouldRefreshSync = nextSynced != _lastSynced;
    final nextButtonBindingSignature =
        _serverSync.daySleepToggleBindingSignature;
    final shouldRefreshButtonBinding =
        nextButtonBindingSignature != _lastButtonBindingSignature;
    final nextButtonDevicesSignature = _buttonDevicesSignature();
    final shouldRefreshButtonDevices =
        nextButtonDevicesSignature != _lastButtonDevicesSignature;

    if (!shouldAdoptTransitions &&
        !shouldRefreshVisuals &&
        !shouldRefreshSync &&
        !shouldRefreshButtonBinding &&
        !shouldRefreshButtonDevices) {
      return;
    }

    setState(() {
      if (shouldAdoptTransitions) {
        _transitionConfigs = nextTransitionConfigs;
        _savedTransitionConfigs = Map.of(nextTransitionConfigs);
        _timeEnabled = nextTimeEnabled;
        _savedTimeEnabled = nextTimeEnabled;
        _handleHours.clear();
      }
      if (shouldRefreshVisuals) {
        _rebuildCurveVisuals();
      }
      if (shouldRefreshButtonBinding) {
        _syncButtonBindingFromServer();
        _lastButtonBindingSignature = nextButtonBindingSignature;
      }
      if (shouldRefreshButtonDevices) {
        _lastButtonDevicesSignature = nextButtonDevicesSignature;
      }
      _lastSynced = nextSynced;
    });
  }

  void _refreshProfileVisualsIfNeeded() {
    final nextSignature = _currentProfileVisualSignature();
    if (nextSignature == _profileVisualSignature) return;

    setState(_rebuildCurveVisuals);
  }

  void _handleHomeChanged() {
    if (!mounted) return;
    final nextKey = _currentSolarLocationKey();
    if (nextKey == _solarLocationKey) return;
    setState(_loadSolarTimes);
  }

  String? _currentSolarLocationKey() {
    final home = context.read<HomeProvider>().currentHome;
    final loc = home?.location;
    if (loc == null) return null;
    return '${loc.latitude}:${loc.longitude}:${home?.timezone ?? ''}';
  }

  Map<RhythmMode, RhythmModeTransitionConfig> _initialTransitionConfigs() {
    return _transitionConfigsFrom(_serverSync.modeTransitions);
  }

  Map<RhythmMode, RhythmModeTransitionConfig> _transitionConfigsFrom(
    Iterable<RhythmModeTransitionConfig> transitions, {
    bool defaultTriggerEnabled = true,
  }) {
    final configs = <RhythmMode, RhythmModeTransitionConfig>{};

    for (final transition in transitions) {
      if (_isSupportedTransition(transition)) {
        configs[transition.toMode] = transition;
      }
    }

    configs.putIfAbsent(
      RhythmMode.day,
      () => _defaultTransitionForMode(RhythmMode.day)
          .copyWith(triggerEnabled: defaultTriggerEnabled),
    );
    configs.putIfAbsent(
      RhythmMode.sleep,
      () => _defaultTransitionForMode(RhythmMode.sleep)
          .copyWith(triggerEnabled: defaultTriggerEnabled),
    );
    return configs;
  }

  bool _timeEnabledFor(Iterable<RhythmModeTransitionConfig> transitions) {
    return transitions.where(_isSupportedTransition).any(
          (transition) =>
              !transition.trigger.isManual && transition.triggerEnabled,
        );
  }

  bool _isSupportedTransition(RhythmModeTransitionConfig transition) {
    return (transition.fromMode == RhythmMode.sleep &&
            transition.toMode == RhythmMode.day) ||
        (transition.fromMode == RhythmMode.day &&
            transition.toMode == RhythmMode.sleep);
  }

  RhythmModeTransitionConfig _defaultTransitionForMode(RhythmMode mode) {
    final isDay = mode == RhythmMode.day;
    return RhythmModeTransitionConfig(
      fromMode: isDay ? RhythmMode.sleep : RhythmMode.day,
      toMode: mode,
      trigger: RhythmTransitionTrigger.solar(isDay ? 'sunrise' : 'sunset'),
      duration: const TransitionDuration.auto(),
      preserveHardOff: true,
    );
  }

  void _loadSolarTimes() {
    final home = context.read<HomeProvider>().currentHome;
    final loc = home?.location;
    _solarLocationKey = _currentSolarLocationKey();
    if (loc == null) {
      _solarClockData = null;
      _curveVisuals.clear();
      _profileVisualSignature = _currentProfileVisualSignature();
      return;
    }
    final tz =
        home?.timezone ?? SolarUtils.timezoneFromLongitude(loc.longitude);
    _solarClockData = computeSolarClockData(
      latitude: loc.latitude,
      longitude: loc.longitude,
      timezone: tz,
    );
    if (_solarClockData == null) {
      _curveVisuals.clear();
      _profileVisualSignature = _currentProfileVisualSignature();
      return;
    }
    _rebuildCurveVisuals(
      latitude: loc.latitude,
      longitude: loc.longitude,
      timezone: tz,
    );
  }

  void _rebuildCurveVisuals({
    double? latitude,
    double? longitude,
    String? timezone,
  }) {
    final home = context.read<HomeProvider>().currentHome;
    final loc = home?.location;
    if (loc == null) {
      _curveVisuals.clear();
      _profileVisualSignature = _currentProfileVisualSignature();
      return;
    }

    _curveVisuals
      ..clear()
      ..addAll(
        _buildModeCurveVisuals(
          latitude: latitude ?? loc.latitude,
          longitude: longitude ?? loc.longitude,
          timezone: timezone ??
              home?.timezone ??
              SolarUtils.timezoneFromLongitude(loc.longitude),
        ),
      );
    _profileVisualSignature = _currentProfileVisualSignature();
  }

  String _currentProfileVisualSignature() {
    final buffer = StringBuffer();
    for (final mode in [RhythmMode.day, RhythmMode.sleep]) {
      final activeProfileId = _activeProfileIdForMode(mode) ?? '';
      final profile = _activeProfileForMode(mode);
      final fallbackColor =
          widget.profileColors[mode] ?? _fallbackModeColor(mode);
      buffer
        ..write(mode.name)
        ..write(':')
        ..write(activeProfileId)
        ..write(':')
        ..write(profile?.hashCode ?? 0)
        ..write(':')
        ..write(_colorSignature(fallbackColor))
        ..write('|');
    }
    return buffer.toString();
  }

  Map<RhythmMode, ModeCurveVisual> _buildModeCurveVisuals({
    required double latitude,
    required double longitude,
    required String timezone,
  }) {
    return {
      for (final mode in [RhythmMode.day, RhythmMode.sleep])
        mode: buildProfileCurveVisual(
          profile: _activeProfileForMode(mode),
          fallbackColor: widget.profileColors[mode] ?? _fallbackModeColor(mode),
          latitude: latitude,
          longitude: longitude,
          timezone: timezone,
        ),
    };
  }

  RhythmCurveConfig? _activeProfileForMode(RhythmMode mode) {
    final activeProfileId = _activeProfileIdForMode(mode);
    if (activeProfileId == null || activeProfileId.isEmpty) return null;

    for (final profile in _serverSync.profiles) {
      if (profile.id == activeProfileId) return profile;
    }
    return null;
  }

  String? _activeProfileIdForMode(RhythmMode mode) {
    for (final modeConfig in _serverSync.modeConfigs) {
      if (modeConfig.mode == mode) {
        return modeConfig.activeProfileId;
      }
    }
    return null;
  }

  void _updateConfig(
    RhythmModeTransitionConfig updated, {
    bool selectUpdatedMode = true,
  }) {
    setState(() {
      _transitionConfigs[updated.toMode] = updated;
      if (selectUpdatedMode) {
        _selectedMode = updated.toMode;
      }
    });
  }

  void _updateSelectedDuration(TransitionDuration duration) {
    // A single duration controls the fade time for BOTH Day and Sleep
    // transitions — apply the new value to both configs so they stay in sync.
    setState(() {
      for (final mode in [RhythmMode.day, RhythmMode.sleep]) {
        final existing = _transitionConfigs[mode];
        if (existing != null) {
          _transitionConfigs[mode] = existing.copyWith(duration: duration);
        }
      }
    });
  }

  /// Commit any pending duration edits to the server inline. Duration lives
  /// outside the Time accordion's save/revert cluster (see [_hasUnsavedChanges])
  /// and instead commits the way the Button section does — silently on the
  /// commit event (slider release / Auto toggle). Sends only the saved
  /// trigger fields alongside the new duration so an in-flight Time edit
  /// doesn't ride along.
  Future<void> _commitDurationInline() async {
    if (!_serverSync.synced) return;
    bool durationDirty = false;
    for (final mode in [RhythmMode.day, RhythmMode.sleep]) {
      final current = _transitionConfigs[mode];
      final saved = _savedTransitionConfigs[mode];
      if (current == null || saved == null) continue;
      if (current.durationMs != saved.durationMs ||
          current.duration.isAuto != saved.duration.isAuto) {
        durationDirty = true;
        break;
      }
    }
    if (!durationDirty) return;

    final merged = <RhythmModeTransitionConfig>[];
    for (final transition in _serverSync.modeTransitions) {
      if (_isSupportedTransition(transition)) {
        final draft = _transitionConfigs[transition.toMode];
        if (draft != null) {
          merged.add(transition.copyWith(duration: draft.duration));
          continue;
        }
      }
      merged.add(transition);
    }

    final success = await _serverSync.dispatchSetTransitions(merged);
    if (!success || !mounted) return;
    setState(() {
      for (final mode in [RhythmMode.day, RhythmMode.sleep]) {
        final current = _transitionConfigs[mode];
        final saved = _savedTransitionConfigs[mode];
        if (current == null || saved == null) continue;
        _savedTransitionConfigs[mode] =
            saved.copyWith(duration: current.duration);
      }
    });
  }

  /// Dirty-state for the save/revert cluster. Scoped strictly to the Time
  /// section — the only edits this cluster governs are the trigger time
  /// (orb drags) and the section's enable toggle. Button-section changes
  /// commit inline via Listen → Confirm; duration changes commit inline on
  /// slider release, so neither participates in this cluster's visibility
  /// or reset behavior.
  bool get _hasUnsavedChanges {
    if (_timeEnabled != _savedTimeEnabled) return true;
    for (final mode in [RhythmMode.day, RhythmMode.sleep]) {
      final current = _transitionConfigs[mode];
      final saved = _savedTransitionConfigs[mode];
      if (current == null || saved == null) {
        if (current != saved) return true;
        continue;
      }
      if (!_timeTriggerEquals(current, saved)) {
        return true;
      }
    }
    return false;
  }

  /// Compares only the Time-accordion fields (the trigger). Duration lives
  /// outside the accordion and is excluded so duration edits don't drive
  /// the save/revert cluster.
  bool _timeTriggerEquals(
    RhythmModeTransitionConfig a,
    RhythmModeTransitionConfig b,
  ) {
    return a.trigger.kind == b.trigger.kind &&
        a.trigger.event == b.trigger.event &&
        a.trigger.time == b.trigger.time;
  }

  void _setTimeEnabled(bool enabled) {
    if (_timeEnabled == enabled) return;
    HapticFeedback.selectionClick();
    setState(() {
      _timeEnabled = enabled;
      // Collapsing the section hides the editor, so any ephemeral orb
      // edits get thrown away — otherwise unsaved changes would linger
      // invisibly inside the closed accordion. Drag state is reset too.
      // Duration lives outside the accordion, so its in-flight edit is
      // preserved.
      if (!enabled) {
        _dragMode = null;
        _dragPreviewHour = null;
        _proximateAnchor = null;
        _anchorProximity = 0.0;
        _revertTimeFieldsToSaved();
        _handleHours.clear();
      }
    });
    unawaited(
      _persistTimeChanges(
        haptic: false,
        showSuccessFeedback: false,
      ),
    );
  }

  /// Restore the saved trigger on each transition config while keeping the
  /// live duration intact. Used by both [_setTimeEnabled] (collapse) and
  /// [_resetChanges] (Revert pill) so they don't clobber duration edits.
  void _revertTimeFieldsToSaved() {
    for (final mode in [RhythmMode.day, RhythmMode.sleep]) {
      final current = _transitionConfigs[mode];
      final saved = _savedTransitionConfigs[mode];
      if (current == null || saved == null) continue;
      _transitionConfigs[mode] = current.copyWith(trigger: saved.trigger);
    }
  }

  void _syncButtonBindingFromServer() {
    final binding = _serverSync.daySleepToggleInputBinding;
    if (_bindState != _ButtonBindState.idle) return;
    _buttonEnabled = binding?.enabled ?? false;
    _toggleButtonDeviceId = binding?.sourceNodeId;
  }

  void _setButtonEnabled(bool enabled) {
    if (_buttonEnabled == enabled) return;
    HapticFeedback.selectionClick();
    setState(() {
      _buttonEnabled = enabled;
      // Collapsing the source aborts an in-flight listen so its ephemeral
      // state can't leak across an enable/disable cycle.
      if (!enabled) {
        _simulatedDetectionTimer?.cancel();
        _listenPulseController.stop();
        _bindState = _ButtonBindState.idle;
        _pendingButton = null;
      }
    });
    unawaited(_serverSync.setDaySleepToggleButtonEnabled(enabled).then((ok) {
      if (!mounted || ok) return;
      setState(() => _syncButtonBindingFromServer());
      _showSaveFeedback('Could not update Button trigger.', error: true);
    }));
  }

  // ── Listen flow ──────────────────────────────────────────────────────────
  // The user taps Listen; the server forwards the next physical press; we
  // surface it as a pending candidate and ask the user to confirm. The
  // backend binding is committed only after confirmation.

  void _startListening() {
    HapticFeedback.lightImpact();
    if (_serverSync.isDemoMode && _buttonDevices.isEmpty) {
      _showSaveFeedback('No button devices found.', error: true);
      return;
    }
    setState(() {
      _bindState = _ButtonBindState.listening;
      _pendingButton = null;
    });
    _listenPulseController
      ..reset()
      ..repeat();
    _simulatedDetectionTimer?.cancel();
    if (_serverSync.isDemoMode) {
      _simulatedDetectionTimer = Timer(const Duration(milliseconds: 2400), () {
        if (!mounted || _bindState != _ButtonBindState.listening) return;
        _onButtonDetected(_simulateServerForwardedPress());
      });
    }
  }

  void _cancelListening() {
    HapticFeedback.selectionClick();
    _simulatedDetectionTimer?.cancel();
    _listenPulseController.stop();
    setState(() {
      _bindState = _ButtonBindState.idle;
      _pendingButton = null;
    });
  }

  void _onButtonDetected(_MockButtonDevice device) {
    HapticFeedback.mediumImpact();
    _simulatedDetectionTimer?.cancel();
    _listenPulseController.stop();
    setState(() {
      _bindState = _ButtonBindState.detected;
      _pendingButton = device;
    });
  }

  void _handleInputEvent(RhythmInputEvent event) {
    if (!mounted || _bindState != _ButtonBindState.listening) return;
    if (event is! RhythmButtonInputEvent) return;
    final sourceNodeId = event.sourceNodeId;
    if (sourceNodeId == null || sourceNodeId.isEmpty) return;

    final known = _resolveButtonDevice(sourceNodeId);
    _onButtonDetected(
      _MockButtonDevice(
        id: sourceNodeId,
        name: known?.name ??
            event.nativeDeviceId ??
            event.nativeButtonId ??
            'Button',
        location: known?.location ?? event.hubType,
        buttonAction: event.buttonAction,
      ),
    );
  }

  void _confirmDetected() {
    final pending = _pendingButton;
    if (pending == null) return;
    HapticFeedback.mediumImpact();
    unawaited(_confirmDetectedAsync(pending));
  }

  Future<void> _confirmDetectedAsync(_MockButtonDevice pending) async {
    final success = await _serverSync.bindDaySleepToggleButton(
      pending.id,
      buttonAction: pending.buttonAction ?? RhythmButtonAction.onPress,
    );
    if (!mounted) return;
    if (!success) {
      _showSaveFeedback('Could not bind Button trigger.', error: true);
      return;
    }
    setState(() {
      _buttonEnabled = true;
      _toggleButtonDeviceId = pending.id;
      _bindState = _ButtonBindState.idle;
      // Keep `_pendingButton` so the summary chips can resolve a name for
      // a device that wasn't in the seed list.
    });
    _lastButtonBindingSignature = _serverSync.daySleepToggleBindingSignature;
  }

  void _clearButton() {
    unawaited(_serverSync.unbindDaySleepToggleButton().then((ok) {
      if (!mounted) return;
      if (!ok) {
        _showSaveFeedback('Could not clear Button trigger.', error: true);
        return;
      }
      setState(() {
        _buttonEnabled = false;
        _toggleButtonDeviceId = null;
        _pendingButton = null;
        _bindState = _ButtonBindState.idle;
      });
      _lastButtonBindingSignature =
          _serverSync.daySleepToggleBindingSignature;
    }));
  }

  _MockButtonDevice _simulateServerForwardedPress() {
    final pool =
        _buttonDevices.where((d) => d.id != _toggleButtonDeviceId).toList();
    if (pool.isNotEmpty) {
      return pool[math.Random().nextInt(pool.length)];
    }
    return _buttonDevices.first;
  }

  _MockButtonDevice? _resolveButtonDevice(String? id) {
    if (id == null) return null;
    for (final d in _buttonDevices) {
      if (d.id == id) return d;
    }
    final pending = _pendingButton;
    if (pending != null && pending.id == id) return pending;
    return null;
  }

  List<_MockButtonDevice> get _buttonDevices {
    return _serverSync.buttonTopologyNodes.map((node) {
      final parentName = node.parentId == null
          ? null
          : _serverSync.topologyNodeById(node.parentId!)?.name;
      return _MockButtonDevice(
        id: node.id,
        name: node.name.isEmpty ? 'Button' : node.name,
        location: parentName,
      );
    }).toList(growable: false);
  }

  String _buttonDevicesSignature() {
    return _serverSync.buttonTopologyNodes
        .map((node) => '${node.id}:${node.name}:${node.parentId ?? ''}')
        .join('|');
  }

  bool _transitionConfigEquals(
    RhythmModeTransitionConfig a,
    RhythmModeTransitionConfig b,
  ) {
    return a.id == b.id &&
        a.fromMode == b.fromMode &&
        a.toMode == b.toMode &&
        a.preserveHardOff == b.preserveHardOff &&
        a.duration.isAuto == b.duration.isAuto &&
        a.durationMs == b.durationMs &&
        a.trigger.kind == b.trigger.kind &&
        a.trigger.event == b.trigger.event &&
        a.trigger.time == b.trigger.time &&
        a.triggerEnabled == b.triggerEnabled;
  }

  bool _transitionConfigMapsEqual(
    Map<RhythmMode, RhythmModeTransitionConfig> a,
    Map<RhythmMode, RhythmModeTransitionConfig> b,
  ) {
    for (final mode in [RhythmMode.day, RhythmMode.sleep]) {
      final left = a[mode];
      final right = b[mode];
      if (left == null || right == null) {
        if (left != right) return false;
        continue;
      }
      if (!_transitionConfigEquals(left, right)) return false;
    }
    return true;
  }

  List<RhythmModeTransitionConfig> _buildTransitionsForSave(
    Map<RhythmMode, RhythmModeTransitionConfig> drafts,
  ) {
    final merged = <RhythmModeTransitionConfig>[];
    final includedModes = <RhythmMode>{};

    for (final transition in _serverSync.modeTransitions) {
      if (_isSupportedTransition(transition)) {
        final replacement = drafts[transition.toMode];
        if (replacement != null) {
          merged.add(replacement.copyWith(triggerEnabled: _timeEnabled));
          includedModes.add(transition.toMode);
          continue;
        }
      }
      merged.add(transition);
    }

    for (final mode in [RhythmMode.day, RhythmMode.sleep]) {
      if (includedModes.contains(mode)) continue;
      final transition = drafts[mode];
      if (transition != null) {
        merged.add(transition.copyWith(triggerEnabled: _timeEnabled));
      }
    }

    return merged;
  }

  Future<void> _saveChanges() => _persistTimeChanges();

  Future<void> _persistTimeChanges({
    bool haptic = true,
    bool showSuccessFeedback = true,
  }) async {
    if (_isSaving || !_hasUnsavedChanges) return;
    if (!_serverSync.synced) {
      _showSaveFeedback('Connect to the server to save changes.', error: true);
      return;
    }

    if (haptic) {
      HapticFeedback.mediumImpact();
    }
    setState(() => _isSaving = true);

    final savedTimeSnapshot = _timeEnabled;
    final transitionsForSave = _buildTransitionsForSave(_transitionConfigs);
    final savedSnapshot = _transitionConfigsFrom(
      transitionsForSave,
      defaultTriggerEnabled: savedTimeSnapshot,
    );
    final success =
        await _serverSync.dispatchSetTransitions(transitionsForSave);

    if (!mounted) return;

    setState(() {
      _isSaving = false;
      if (success) {
        _transitionConfigs = Map.of(savedSnapshot);
        _savedTransitionConfigs = savedSnapshot;
        _savedTimeEnabled = savedTimeSnapshot;
      }
    });

    if (showSuccessFeedback || !success) {
      _showSaveFeedback(
        success ? 'Transitions saved.' : 'Could not save Transitions.',
        error: !success,
      );
    }
  }

  /// Throws away in-flight Time-section edits and returns to the last saved
  /// snapshot. Intentionally does not touch Button-section state — that
  /// section commits inline through its own Listen → Confirm flow. Duration
  /// also commits inline (on slider release / Auto toggle) and lives
  /// outside this accordion, so it is preserved across a Revert too.
  void _resetChanges() {
    if (!_hasUnsavedChanges) return;
    HapticFeedback.mediumImpact();
    setState(() {
      _revertTimeFieldsToSaved();
      _timeEnabled = _savedTimeEnabled;
      _handleHours.clear();
      _dragMode = null;
      _dragPreviewHour = null;
      _proximateAnchor = null;
      _anchorProximity = 0.0;
    });
  }

  void _showSaveFeedback(String message, {required bool error}) {
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(message),
        behavior: SnackBarBehavior.floating,
        backgroundColor:
            error ? const Color(0xFF7A2E2E) : CelestialColors.backgroundCard,
      ),
    );
  }

  void _focusMode(RhythmMode mode) {
    if (_selectedMode == mode) return;
    HapticFeedback.selectionClick();
    setState(() => _selectedMode = mode);
  }

  void _setModeHour(
    RhythmMode mode,
    double hour, {
    TriggerAnchor? snappedAnchor,
  }) {
    final current = _transitionConfigs[mode];
    if (current == null) return;
    final normalizedHour = SolarUtils.normalizeHour(hour);

    if (snappedAnchor != null) {
      final needsSolarUpdate = !current.trigger.isSolar ||
          current.trigger.event != snappedAnchor.event;
      final hadLocalHour = _handleHours.containsKey(mode);

      if (!needsSolarUpdate && !hadLocalHour) {
        if (_selectedMode != mode) {
          setState(() => _selectedMode = mode);
        }
        return;
      }

      setState(() {
        _selectedMode = mode;
        _handleHours.remove(mode);
      });

      if (needsSolarUpdate) {
        _updateConfig(
          current.copyWith(
            trigger: RhythmTransitionTrigger.solar(snappedAnchor.event),
          ),
        );
      }
      return;
    }

    final scheduledTime = _formatScheduledTriggerTime(normalizedHour);
    final scheduledHour = _scheduledTimeToHour(scheduledTime) ?? normalizedHour;
    final previousHour = _handleHourForMode(mode);
    final hourChanged =
        previousHour == null || (previousHour - scheduledHour).abs() > 0.001;
    final needsTriggerUpdate =
        !current.trigger.isScheduled || current.trigger.time != scheduledTime;

    if (!hourChanged && !needsTriggerUpdate) {
      if (_selectedMode != mode) {
        setState(() => _selectedMode = mode);
      }
      return;
    }

    setState(() {
      _selectedMode = mode;
      _handleHours[mode] = scheduledHour;
    });

    if (needsTriggerUpdate) {
      _updateConfig(
        current.copyWith(
            trigger: RhythmTransitionTrigger.scheduled(scheduledTime)),
      );
    }
  }

  @override
  Widget build(BuildContext context) {
    // Read entitlements up here — `context.select` is not safe to call from
    // inside a LayoutBuilder's builder (it runs during layout, not build),
    // and `_buildButtonSection` is reached through one.
    final canUseTransitionButton = context.select<SubscriptionProvider, bool>(
      (s) => s.has(Entitlement.transitionButton),
    );
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            _buildDetailHeader(context),
            Expanded(
              child: LayoutBuilder(
                builder: (context, constraints) {
                  final compactLayout = constraints.maxHeight < 680;
                  final topSpacing = compactLayout ? 2.0 : 6.0;
                  final sectionSpacing = compactLayout ? 12.0 : 16.0;
                  final bottomSpacing = compactLayout ? 14.0 : 18.0;
                  final isAutomatic =
                      widget.detail == AutomationDetail.automatic;
                  final dayColor = widget.profileColors[RhythmMode.day] ??
                      _fallbackModeColor(RhythmMode.day);
                  final sleepColor = widget.profileColors[RhythmMode.sleep] ??
                      _fallbackModeColor(RhythmMode.sleep);

                  // Whole page scrolls — the single section expands to its
                  // natural content height with no inner scrolling. The
                  // transition-duration control is shown for both automations.
                  return SingleChildScrollView(
                    padding: const EdgeInsets.symmetric(horizontal: 20),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        SizedBox(height: topSpacing),
                        if (isAutomatic) ...[
                          _buildTimeSection(
                            dayColor: dayColor,
                            sleepColor: sleepColor,
                            compactLayout: compactLayout,
                          ),
                          _buildSaveResetSlot(),
                        ] else
                          _buildButtonSection(
                            accentColor: _chromeAccent,
                            compactLayout: compactLayout,
                            unlocked: canUseTransitionButton,
                          ),
                        SizedBox(height: sectionSpacing),
                        _buildDurationRow(compactLayout: compactLayout),
                        SizedBox(height: bottomSpacing),
                      ],
                    ),
                  );
                },
              ),
            ),
          ],
        ),
      ),
    );
  }

  String get _detailTitle => switch (widget.detail) {
        AutomationDetail.automatic => 'Alarm Schedule',
        AutomationDetail.button => 'Wake/Sleep Button',
      };

  Widget _buildDetailHeader(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
      child: Row(
        children: [
          GestureDetector(
            onTap: () => Navigator.of(context).pop(),
            child: Container(
              width: 40,
              height: 40,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: CelestialColors.accentBlue.withValues(alpha: 0.2),
              ),
              child: const Icon(
                Icons.chevron_left,
                color: CelestialColors.accentBlue,
                size: 24,
              ),
            ),
          ),
          Expanded(
            child: Text(
              _detailTitle,
              textAlign: TextAlign.center,
              style: const TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 18,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
          const SizedBox(width: 40),
        ],
      ),
    );
  }

  /// Save + Reset cluster anchored to the bottom of the Time section's
  /// expanded body. Quiet pair of pills — only mount when there are
  /// unsaved edits, regardless of the active source. Save commits every
  /// change on the page; Reset throws every change away.
  Widget _buildSaveResetSlot() {
    // Reading `_serverSync` directly (rather than `context.select`) avoids a
    // Provider assertion when this method runs inside a LayoutBuilder
    // builder. `_handleServerSyncChanged` calls setState whenever
    // `_serverSync.synced` flips, so the pill stays in sync.
    final synced = _serverSync.synced;
    final showCluster = _hasUnsavedChanges || _isSaving;
    return AnimatedSize(
      duration: const Duration(milliseconds: 220),
      curve: Curves.easeOutCubic,
      alignment: Alignment.topCenter,
      child: AnimatedSwitcher(
        duration: const Duration(milliseconds: 220),
        switchInCurve: Curves.easeOutCubic,
        switchOutCurve: Curves.easeInCubic,
        transitionBuilder: (child, animation) {
          return FadeTransition(
            opacity: animation,
            child: ScaleTransition(
              scale: Tween<double>(begin: 0.92, end: 1.0).animate(animation),
              child: child,
            ),
          );
        },
        child: showCluster
            ? Padding(
                key: const ValueKey('save-reset-on'),
                padding: const EdgeInsets.fromLTRB(4, 6, 4, 4),
                child: Row(
                  mainAxisAlignment: MainAxisAlignment.end,
                  children: [
                    _ResetPill(
                      accentColor: _chromeAccent,
                      onTap: _isSaving ? null : _resetChanges,
                    ),
                    const SizedBox(width: 8),
                    _SavePill(
                      accentColor: _chromeAccent,
                      isSaving: _isSaving,
                      synced: synced,
                      onTap: (synced && !_isSaving) ? _saveChanges : null,
                    ),
                  ],
                ),
              )
            : const SizedBox(
                key: ValueKey('save-reset-off'),
                width: double.infinity,
                height: 0,
              ),
      ),
    );
  }

  /// Schedule-based trigger section. Header with an enable switch; when on,
  /// the orbital editor + reset pill expand below. Section card visibly
  /// "powers down" when the switch is off.
  Widget _buildTimeSection({
    required Color dayColor,
    required Color sleepColor,
    required bool compactLayout,
  }) {
    return _SourceSectionCard(
      enabled: _timeEnabled,
      accent: _chromeAccent,
      header: _SourceSectionHeader(
        icon: Icons.access_time_rounded,
        label: 'Schedule',
        sublabel:
            _timeEnabled ? 'Day & Sleep follow the sun & clock' : 'Off',
        accent: _chromeAccent,
        enabled: _timeEnabled,
        onChanged: _setTimeEnabled,
        tooltip:
            'Automatically switch between Day and Sleep at chosen times — '
            'either anchored to solar events (sunrise, sunset) or fixed clock '
            'times. Drag the orbs to adjust.',
      ),
      body: Padding(
        padding: const EdgeInsets.fromLTRB(8, 4, 8, 12),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(4, 4, 4, 4),
              child: _buildSourceSummaryChips(
                dayColor: dayColor,
                sleepColor: sleepColor,
              ),
            ),
            AspectRatio(
              aspectRatio: 1.0,
              child: _solarClockData != null
                  ? _buildInteractiveTransitionFlow(
                      dayColor: dayColor,
                      sleepColor: sleepColor,
                    )
                  : _buildNoSolarState(),
            ),
          ],
        ),
      ),
    );
  }

  /// Physical-button trigger section. Header with an enable switch; when on,
  /// the device picker + pair CTA expand below. Gated by the
  /// [Entitlement.transitionButton] entitlement — for users without it the
  /// header renders a Pro pill in place of the switch and tapping the header
  /// opens the upsell modal.
  Widget _buildButtonSection({
    required Color accentColor,
    required bool compactLayout,
    required bool unlocked,
  }) {
    final header = _SourceSectionHeader(
      icon: Icons.radio_button_checked_rounded,
      label: 'Button',
      sublabel: !unlocked
          ? 'Pro · Bind a physical button'
          : _buttonEnabled
              ? 'One button toggles Day ⇄ Sleep'
              : 'Off',
      accent: _chromeAccent,
      enabled: unlocked && _buttonEnabled,
      onChanged: _setButtonEnabled,
      onLockTap: unlocked
          ? null
          : () => PlanTierModal.show(
                context,
                highlightFeature: Entitlement.transitionButton,
              ),
      tooltip:
          'Bind a physical button (e.g. a Hue Tap or Dimmer) so a single '
          'press toggles between Day and Sleep mode. Great as a bedside '
          'sleep + wake button.',
    );

    return _SourceSectionCard(
      enabled: unlocked && _buttonEnabled,
      accent: _chromeAccent,
      header: header,
      body: unlocked ? _buildButtonSectionBody() : const SizedBox.shrink(),
    );
  }

  Widget _buildButtonSectionBody() {
    return Padding(
      padding: const EdgeInsets.fromLTRB(10, 4, 10, 14),
      child: _ButtonListenStage(
        bindState: _bindState,
        pulse: _listenPulseController,
        accent: _chromeAccent,
        boundDevice: _resolveButtonDevice(_toggleButtonDeviceId),
        pendingDevice: _pendingButton,
        onListen: _startListening,
        onCancel: _cancelListening,
        onRetry: _startListening,
        onConfirm: _confirmDetected,
        onClear: _clearButton,
      ),
    );
  }

  Widget _buildInteractiveTransitionFlow({
    required Color dayColor,
    required Color sleepColor,
  }) {
    final solarClockData = _solarClockData!;
    final dayConfig = _transitionConfigs[RhythmMode.day];
    final sleepConfig = _transitionConfigs[RhythmMode.sleep];

    return RhythmScheduleClock(
      data: solarClockData,
      dayHour: _handleHourForMode(RhythmMode.day) ??
          solarClockData.sunTimes.sunrise,
      sleepHour: _handleHourForMode(RhythmMode.sleep) ??
          solarClockData.sunTimes.sunset,
      dayColor: dayColor,
      sleepColor: sleepColor,
      dayVisual:
          _curveVisuals[RhythmMode.day] ?? ModeCurveVisual(fallbackColor: dayColor),
      sleepVisual: _curveVisuals[RhythmMode.sleep] ??
          ModeCurveVisual(fallbackColor: sleepColor),
      selectedMode: _selectedMode,
      onModeSelected: _focusMode,
      dayAnchors: solarAnchorsForMode(RhythmMode.day, solarClockData),
      sleepAnchors: solarAnchorsForMode(RhythmMode.sleep, solarClockData),
      activeDayEvent:
          dayConfig?.trigger.isSolar == true ? dayConfig?.trigger.event : null,
      activeSleepEvent: sleepConfig?.trigger.isSolar == true
          ? sleepConfig?.trigger.event
          : null,
      onHourCommitted: (mode, hour, snapped) =>
          _setModeHour(mode, hour, snappedAnchor: snapped),
      onDragPreview: (preview) => setState(() {
        _dragMode = preview?.mode;
        _dragPreviewHour = preview?.hour;
        _proximateAnchor = preview?.proximateAnchor;
        _anchorProximity = preview?.proximity ?? 0.0;
      }),
    );
  }

  Widget _buildNoSolarState() {
    return Center(
      child: Text(
        'Set a home location to unlock the solar rhythm editor.',
        textAlign: TextAlign.center,
        style: TextStyle(
          color: CelestialColors.textSecondary.withValues(alpha: 0.7),
          fontSize: 14,
          fontWeight: FontWeight.w500,
        ),
      ),
    );
  }

  /// Pair of summary chips for the TIME source, rendered inside the time
  /// accordion.
  Widget _buildSourceSummaryChips({
    required Color dayColor,
    required Color sleepColor,
  }) {
    return IntrinsicHeight(
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Expanded(
            child: _buildModeSummaryChip(
              mode: RhythmMode.day,
              modeColor: dayColor,
            ),
          ),
          const SizedBox(width: 8),
          Expanded(
            child: _buildModeSummaryChip(
              mode: RhythmMode.sleep,
              modeColor: sleepColor,
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildModeSummaryChip({
    required RhythmMode mode,
    required Color modeColor,
  }) {
    final config = _transitionConfigs[mode];
    if (config == null) return const SizedBox.shrink();
    final isDraggingThis = _dragMode == mode;

    String triggerLabel;
    double? triggerHour;
    if (isDraggingThis && _proximateAnchor != null && _anchorProximity > 0.5) {
      triggerLabel = _shortAnchorLabel(_proximateAnchor!);
      triggerHour = _proximateAnchor!.hour;
    } else if (isDraggingThis) {
      triggerLabel = '';
      triggerHour = _dragPreviewHour;
    } else {
      triggerLabel = _shortTriggerLabel(config.trigger);
      triggerHour = _triggerTimeHoursForMode(mode);
    }
    final timeText = triggerHour != null ? _fmtTime(triggerHour) : null;
    final String valueLine;
    if (triggerLabel.isNotEmpty && timeText != null) {
      valueLine = '$triggerLabel  ·  $timeText';
    } else {
      valueLine = timeText ?? triggerLabel;
    }

    return ModeSummaryChip(
      icon: _modeIcon(mode),
      label: '${_modeLabel(mode)} Start',
      accent: modeColor,
      child: Text(
        valueLine,
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
        style: TextStyle(
          color: CelestialColors.textPrimary.withValues(alpha: 0.88),
          fontSize: 12.5,
          fontWeight: FontWeight.w600,
          letterSpacing: 0.1,
        ),
      ),
    );
  }


  double? _handleHourForMode(RhythmMode mode) {
    final draggedHour = _handleHours[mode];
    if (draggedHour != null) return draggedHour;

    final config = _transitionConfigs[mode];
    final scheduledTime = config?.trigger.time;
    if (config?.trigger.isScheduled == true && scheduledTime != null) {
      final scheduledHour = _scheduledTimeToHour(scheduledTime);
      if (scheduledHour != null) return scheduledHour;
    }

    final triggerEvent = config?.trigger.event;
    if (config?.trigger.isSolar == true && triggerEvent != null) {
      final eventHour = _eventTimeHours(mode, triggerEvent);
      if (eventHour != null) return eventHour;
    }

    return switch (mode) {
      RhythmMode.day => _sunTimes?.sunrise,
      RhythmMode.sleep => _sunTimes?.sunset,
    };
  }


  double? _triggerTimeHoursForMode(RhythmMode mode) {
    final config = _transitionConfigs[mode];
    if (config == null) return null;
    if (config.trigger.isScheduled) {
      final scheduledTime = config.trigger.time;
      if (scheduledTime != null) {
        return _scheduledTimeToHour(scheduledTime);
      }
    }
    if (config.trigger.kind == 'manual') {
      return _handleHourForMode(mode);
    }
    final event = config.trigger.event;
    if (event == null || config.trigger.kind != 'solar') return null;
    return _eventTimeHours(mode, event);
  }

  double? _eventTimeHours(RhythmMode mode, String event) {
    final isDawn = mode == RhythmMode.day;
    return switch (event) {
      'sunrise' => _sunTimes?.sunrise,
      'sunset' => _sunTimes?.sunset,
      'civil_twilight' =>
        isDawn ? _twilightTimes?.dawn.civil : _twilightTimes?.dusk.civil,
      'nautical_twilight' =>
        isDawn ? _twilightTimes?.dawn.nautical : _twilightTimes?.dusk.nautical,
      'astronomical_twilight' => isDawn
          ? _twilightTimes?.dawn.astronomical
          : _twilightTimes?.dusk.astronomical,
      _ => null,
    };
  }

  String _fmtTime(double h) {
    return SolarUtils.formatHour(
      h,
      use24: MediaQuery.alwaysUse24HourFormatOf(context),
    );
  }

  bool get _durationAuto => _config.duration.isAuto;

  int get _fineDurationSteps =>
      ((_bendDurationSeconds - _minDurationSeconds) / _fineDurationStepSeconds)
          .round();
  int get _coarseDurationSteps =>
      ((_maxDurationSeconds - _bendDurationSeconds) /
              _coarseDurationStepSeconds)
          .round();
  int get _totalDurationSteps => _fineDurationSteps + _coarseDurationSteps;

  double _durationStepToSeconds(int step) {
    if (step <= _fineDurationSteps) {
      return _minDurationSeconds + step * _fineDurationStepSeconds;
    }
    return _bendDurationSeconds +
        (step - _fineDurationSteps) * _coarseDurationStepSeconds;
  }

  int _durationSecondsToStep(double seconds) {
    if (seconds <= _bendDurationSeconds) {
      return ((seconds - _minDurationSeconds) / _fineDurationStepSeconds)
          .round()
          .clamp(0, _fineDurationSteps);
    }
    return (_fineDurationSteps +
            (seconds - _bendDurationSeconds) / _coarseDurationStepSeconds)
        .round()
        .clamp(0, _totalDurationSteps);
  }

  Widget _buildDurationRow({required bool compactLayout}) {
    final accentColor = widget.profileColors[_selectedMode] ??
        _fallbackModeColor(_selectedMode);
    final modeKey = _selectedMode.name;
    final clampedDurationSeconds = (_config.durationMs / 1000.0)
        .clamp(_minDurationSeconds, _maxDurationSeconds)
        .toDouble();
    final sliderStep = _durationSecondsToStep(clampedDurationSeconds);

    return Container(
      padding: EdgeInsets.fromLTRB(
          20, compactLayout ? 12 : 14, 16, compactLayout ? 12 : 14),
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(20),
        border: Border.all(color: _hairlineSoft, width: 1),
      ),
      child: Column(
        children: [
          Row(
            key: ValueKey('duration-header-$modeKey'),
            children: [
              Container(
                width: 30,
                height: 30,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: accentColor.withValues(alpha: 0.1),
                ),
                child: Icon(Icons.timer_outlined, color: accentColor, size: 15),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Row(
                  children: [
                    const Flexible(
                      child: Text(
                        'Transition Duration',
                        style: TextStyle(
                          color: CelestialColors.textPrimary,
                          fontSize: 14,
                          fontWeight: FontWeight.w500,
                        ),
                      ),
                    ),
                    const SizedBox(width: 4),
                    const InfoTooltip(
                      message:
                          'Applies to both Day and Sleep transitions. This is the light fade time of the transition.',
                    ),
                  ],
                ),
              ),
              GestureDetector(
                onTap: () {
                  HapticFeedback.selectionClick();
                  if (_durationAuto) {
                    _updateSelectedDuration(
                      const TransitionDuration.fixed(30000),
                    );
                  } else {
                    _updateSelectedDuration(const TransitionDuration.auto());
                  }
                  unawaited(_commitDurationInline());
                },
                child: Container(
                  padding:
                      const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
                  decoration: BoxDecoration(
                    color: _durationAuto
                        ? accentColor.withValues(alpha: 0.12)
                        : accentColor.withValues(alpha: 0.06),
                    borderRadius: BorderRadius.circular(6),
                    border: Border.all(
                      color: _durationAuto
                          ? accentColor.withValues(alpha: 0.25)
                          : accentColor.withValues(alpha: 0.12),
                    ),
                  ),
                  child: Text(
                    _durationAuto
                        ? 'Auto'
                        : _formatDuration(_config.durationMs),
                    style: TextStyle(
                      color: _durationAuto
                          ? accentColor
                          : accentColor.withValues(alpha: 0.7),
                      fontSize: 11,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ),
              ),
            ],
          ),
          AnimatedSize(
            key: ValueKey('duration-body-$modeKey'),
            duration: const Duration(milliseconds: 250),
            curve: Curves.easeOutCubic,
            alignment: Alignment.topCenter,
            child: _durationAuto
                ? const SizedBox.shrink()
                : Padding(
                    padding: EdgeInsets.only(top: compactLayout ? 6 : 8),
                    child: Column(
                      children: [
                        SliderTheme(
                          data: SliderThemeData(
                            activeTrackColor: accentColor,
                            inactiveTrackColor:
                                accentColor.withValues(alpha: 0.12),
                            // Tick marks are suppressed because the piecewise
                            // step count (~56) would otherwise stripe the
                            // track. The bend point is communicated via the
                            // end labels instead.
                            tickMarkShape: SliderTickMarkShape.noTickMark,
                            thumbColor: accentColor,
                            overlayColor: accentColor.withValues(alpha: 0.12),
                            trackHeight: 4,
                            thumbShape: const RoundSliderThumbShape(
                              enabledThumbRadius: 8,
                            ),
                            overlayShape: const RoundSliderOverlayShape(
                              overlayRadius: 18,
                            ),
                          ),
                          child: Slider(
                            key: ValueKey('duration-slider-$modeKey'),
                            value: sliderStep.toDouble(),
                            min: 0,
                            max: _totalDurationSteps.toDouble(),
                            divisions: _totalDurationSteps,
                            onChanged: (v) {
                              final seconds = _durationStepToSeconds(v.round());
                              _updateSelectedDuration(
                                TransitionDuration.fixed(
                                  (seconds * 1000).round(),
                                ),
                              );
                            },
                            onChangeEnd: (_) {
                              unawaited(_commitDurationInline());
                            },
                          ),
                        ),
                        Padding(
                          padding: const EdgeInsets.symmetric(horizontal: 6),
                          child: Row(
                            children: [
                              Text(
                                '1s',
                                style: TextStyle(
                                  color: CelestialColors.textSecondary
                                      .withValues(alpha: 0.4),
                                  fontSize: 11,
                                ),
                              ),
                              const Spacer(),
                              // Bend hint sits at the 30s waypoint to signal
                              // the granular→coarse handoff without on-track
                              // tick marks.
                              Text(
                                '30s',
                                style: TextStyle(
                                  color: CelestialColors.textSecondary
                                      .withValues(alpha: 0.35),
                                  fontSize: 11,
                                ),
                              ),
                              const Spacer(),
                              Text(
                                '5m',
                                style: TextStyle(
                                  color: CelestialColors.textSecondary
                                      .withValues(alpha: 0.4),
                                  fontSize: 11,
                                ),
                              ),
                            ],
                          ),
                        ),
                      ],
                    ),
                  ),
          ),
        ],
      ),
    );
  }
}

/// State machine for the Button section's Listen flow. `idle` covers both
/// "nothing bound yet" and "bound, resting" — the bound device is what
/// disambiguates them in the UI.
enum _ButtonBindState { idle, listening, detected }

/// Page-local view model for button devices surfaced by server topology.
class _MockButtonDevice {
  final String id;
  final String name;
  final String? location;
  final RhythmButtonAction? buttonAction;

  const _MockButtonDevice({
    required this.id,
    required this.name,
    this.location,
    this.buttonAction,
  });
}

/// Card-style sub-section inside the hero, with a header (icon + label +
/// switch) and an animated body that collapses when [enabled] is false.
class _SourceSectionCard extends StatelessWidget {
  final bool enabled;
  final Color accent;
  final Widget header;
  final Widget body;

  const _SourceSectionCard({
    required this.enabled,
    required this.accent,
    required this.header,
    required this.body,
  });

  @override
  Widget build(BuildContext context) {
    return AnimatedContainer(
      duration: const Duration(milliseconds: 220),
      curve: Curves.easeOutCubic,
      decoration: BoxDecoration(
        color: enabled ? _surfaceElevated : _surfaceRecessed,
        borderRadius: BorderRadius.circular(16),
        border: Border.all(
          color: enabled ? _hairlineStrong : _hairlineSoft,
          width: 1,
        ),
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          header,
          AnimatedSize(
            duration: const Duration(milliseconds: 220),
            curve: Curves.easeOutCubic,
            alignment: Alignment.topCenter,
            child: enabled
                ? body
                : const SizedBox(width: double.infinity, height: 0),
          ),
        ],
      ),
    );
  }
}

/// Row that opens each [_SourceSectionCard]. Tap anywhere on the row to
/// toggle the source on/off; the switch is the obvious affordance.
class _SourceSectionHeader extends StatelessWidget {
  final IconData icon;
  final String label;
  final String sublabel;
  final Color accent;
  final bool enabled;
  final ValueChanged<bool> onChanged;

  /// When set, the section is rendered as a Pro-locked teaser: tapping
  /// anywhere on the header fires [onLockTap] instead of toggling, and the
  /// right side renders an amber Pro pill instead of the enable switch.
  final VoidCallback? onLockTap;

  /// Optional explanatory tooltip rendered as an (i) chip next to [label].
  final String? tooltip;

  const _SourceSectionHeader({
    required this.icon,
    required this.label,
    required this.sublabel,
    required this.accent,
    required this.enabled,
    required this.onChanged,
    this.onLockTap,
    this.tooltip,
  });

  bool get _locked => onLockTap != null;

  @override
  Widget build(BuildContext context) {
    final activeColor = _locked
        ? const Color(0xFFFFB900).withValues(alpha: 0.85)
        : enabled
            ? accent.withValues(alpha: 0.92)
            : CelestialColors.textSecondary.withValues(alpha: 0.6);
    return GestureDetector(
      behavior: HitTestBehavior.opaque,
      onTap: () {
        if (_locked) {
          onLockTap!();
        } else {
          onChanged(!enabled);
        }
      },
      child: Padding(
        padding: const EdgeInsets.fromLTRB(14, 12, 12, 12),
        child: Row(
          children: [
            // Glyph in a soft accent disc when enabled.
            AnimatedContainer(
              duration: const Duration(milliseconds: 220),
              width: 30,
              height: 30,
              alignment: Alignment.center,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: _locked
                    ? const Color(0xFFFFB900).withValues(alpha: 0.10)
                    : enabled
                        ? CelestialColors.backgroundCard
                        : _surfaceRecessed,
                border: Border.all(
                  color: _locked
                      ? const Color(0xFFFFB900).withValues(alpha: 0.35)
                      : enabled
                          ? _hairlineStrong
                          : _hairlineSoft,
                ),
              ),
              child: Icon(icon, size: 15, color: activeColor),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisSize: MainAxisSize.min,
                children: [
                  Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Flexible(
                        child: AnimatedDefaultTextStyle(
                          duration: const Duration(milliseconds: 200),
                          style: TextStyle(
                            color: enabled
                                ? CelestialColors.textPrimary
                                : CelestialColors.textPrimary
                                    .withValues(alpha: 0.6),
                            fontSize: 15,
                            fontWeight: FontWeight.w700,
                            letterSpacing: 0.3,
                          ),
                          child: Text(label, overflow: TextOverflow.ellipsis),
                        ),
                      ),
                      if (tooltip != null) ...[
                        const SizedBox(width: 4),
                        InfoTooltip(message: tooltip!, iconSize: 13),
                      ],
                    ],
                  ),
                  const SizedBox(height: 2),
                  AnimatedDefaultTextStyle(
                    duration: const Duration(milliseconds: 200),
                    style: TextStyle(
                      color: enabled
                          ? CelestialColors.textSecondary
                              .withValues(alpha: 0.75)
                          : CelestialColors.textSecondary
                              .withValues(alpha: 0.5),
                      fontSize: 11.5,
                      fontWeight: FontWeight.w500,
                      letterSpacing: 0.2,
                    ),
                    child: Text(sublabel),
                  ),
                ],
              ),
            ),
            if (_locked)
              const _HeaderProPill()
            else
              _CelestialSwitch(
                value: enabled,
                accent: accent,
                onChanged: onChanged,
              ),
          ],
        ),
      ),
    );
  }
}

/// Solid amber pill that replaces the section switch when a section is
/// Pro-locked. Tap is handled by the parent header's gesture detector — this
/// is purely decorative.
class _HeaderProPill extends StatelessWidget {
  const _HeaderProPill();

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
      decoration: BoxDecoration(
        gradient: const LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [Color(0xFFFFB900), Color(0xFFFF8C00)],
        ),
        borderRadius: BorderRadius.circular(999),
        boxShadow: [
          BoxShadow(
            color: const Color(0xFFFFB900).withValues(alpha: 0.32),
            blurRadius: 10,
            spreadRadius: -2,
          ),
        ],
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: const [
          Icon(Icons.lock_open_rounded, size: 11, color: Colors.white),
          SizedBox(width: 4),
          Text(
            'PRO',
            style: TextStyle(
              color: Colors.white,
              fontSize: 10,
              fontWeight: FontWeight.w800,
              letterSpacing: 1.0,
            ),
          ),
        ],
      ),
    );
  }
}

/// Pill-shaped toggle switch tuned to the celestial palette. Accent fill +
/// soft glow when on, dark with a hairline outline when off.
class _CelestialSwitch extends StatelessWidget {
  final bool value;
  final Color accent;
  final ValueChanged<bool> onChanged;

  const _CelestialSwitch({
    required this.value,
    required this.accent,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    const trackWidth = 44.0;
    const trackHeight = 26.0;
    const thumbSize = 20.0;
    final thumbOffset = value ? (trackWidth - thumbSize - 3.0) : 3.0;
    return GestureDetector(
      behavior: HitTestBehavior.opaque,
      onTap: () => onChanged(!value),
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 220),
        curve: Curves.easeOutCubic,
        width: trackWidth,
        height: trackHeight,
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(trackHeight / 2),
          color: value ? accent.withValues(alpha: 0.20) : _surfaceRecessed,
          border: Border.all(
            color: value ? accent.withValues(alpha: 0.45) : _hairlineStrong,
            width: 1,
          ),
          boxShadow: [
            if (value)
              BoxShadow(
                color: accent.withValues(alpha: 0.22),
                blurRadius: 12,
                spreadRadius: -2,
              ),
          ],
        ),
        child: Stack(
          children: [
            AnimatedPositioned(
              duration: const Duration(milliseconds: 220),
              curve: Curves.easeOutCubic,
              top: 2,
              left: thumbOffset,
              child: AnimatedContainer(
                duration: const Duration(milliseconds: 220),
                width: thumbSize,
                height: thumbSize,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: value
                      ? accent.withValues(alpha: 0.92)
                      : const Color(0xFF4A5260),
                  boxShadow: [
                    if (value)
                      BoxShadow(
                        color: accent.withValues(alpha: 0.35),
                        blurRadius: 8,
                      ),
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Listen-flow stage
//
// Four states share a single morphing surface: unbound (no button yet),
// listening (server forwarding the next press), detected (candidate awaiting
// confirmation), bound (a button is committed). AnimatedSwitcher + AnimatedSize
// let the section breathe between states without snapping.
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

class _ButtonListenStage extends StatelessWidget {
  final _ButtonBindState bindState;
  final Animation<double> pulse;
  final Color accent;
  final _MockButtonDevice? boundDevice;
  final _MockButtonDevice? pendingDevice;
  final VoidCallback onListen;
  final VoidCallback onCancel;
  final VoidCallback onRetry;
  final VoidCallback onConfirm;
  final VoidCallback onClear;

  const _ButtonListenStage({
    required this.bindState,
    required this.pulse,
    required this.accent,
    required this.boundDevice,
    required this.pendingDevice,
    required this.onListen,
    required this.onCancel,
    required this.onRetry,
    required this.onConfirm,
    required this.onClear,
  });

  @override
  Widget build(BuildContext context) {
    return AnimatedSize(
      duration: const Duration(milliseconds: 280),
      curve: Curves.easeOutCubic,
      alignment: Alignment.topCenter,
      child: AnimatedSwitcher(
        duration: const Duration(milliseconds: 240),
        switchInCurve: Curves.easeOutCubic,
        switchOutCurve: Curves.easeInCubic,
        transitionBuilder: (child, animation) {
          return FadeTransition(
            opacity: animation,
            child: SlideTransition(
              position: Tween<Offset>(
                begin: const Offset(0, 0.04),
                end: Offset.zero,
              ).animate(animation),
              child: child,
            ),
          );
        },
        child: _buildState(),
      ),
    );
  }

  Widget _buildState() {
    switch (bindState) {
      case _ButtonBindState.listening:
        return _ListenStateListening(
          key: const ValueKey('listening'),
          pulse: pulse,
          accent: accent,
          onCancel: onCancel,
        );
      case _ButtonBindState.detected:
        return _ListenStateDetected(
          key: const ValueKey('detected'),
          accent: accent,
          device: pendingDevice,
          onRetry: onRetry,
          onConfirm: onConfirm,
        );
      case _ButtonBindState.idle:
        final bound = boundDevice;
        if (bound != null) {
          return _ListenStateBound(
            key: const ValueKey('bound'),
            accent: accent,
            device: bound,
            onListen: onListen,
            onClear: onClear,
          );
        }
        return _ListenStateUnbound(
          key: const ValueKey('unbound'),
          accent: accent,
          onListen: onListen,
        );
    }
  }
}

/// Idle state with no committed button. A quiet sonar stage invites the user
/// to start listening.
class _ListenStateUnbound extends StatelessWidget {
  final Color accent;
  final VoidCallback onListen;

  const _ListenStateUnbound({
    super.key,
    required this.accent,
    required this.onListen,
  });

  @override
  Widget build(BuildContext context) {
    return Column(
      key: const ValueKey('unbound-col'),
      mainAxisSize: MainAxisSize.min,
      children: [
        Padding(
          padding: const EdgeInsets.only(bottom: 10),
          child: Text(
            'Tap Listen, then press a button on your remote.',
            textAlign: TextAlign.center,
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.55),
              fontSize: 11.5,
              fontWeight: FontWeight.w500,
              letterSpacing: 0.2,
            ),
          ),
        ),
        _ListenPrimaryCta(
          accent: accent,
          icon: Icons.sensors_rounded,
          label: 'Listen for a press',
          onTap: onListen,
        ),
      ],
    );
  }
}

/// Active listen state — radar pulses outward while we wait for the server
/// to forward the next button press.
class _ListenStateListening extends StatelessWidget {
  final Animation<double> pulse;
  final Color accent;
  final VoidCallback onCancel;

  const _ListenStateListening({
    super.key,
    required this.pulse,
    required this.accent,
    required this.onCancel,
  });

  @override
  Widget build(BuildContext context) {
    return Column(
      key: const ValueKey('listening-col'),
      mainAxisSize: MainAxisSize.min,
      children: [
        _RadarStage(
          accent: accent,
          pulse: pulse,
          mode: _RadarStageMode.scanning,
          diameter: 140,
        ),
        const SizedBox(height: 10),
        _ListenCaption(
          accent: accent,
          title: 'Listening…',
          subtitle: 'Press any button on your remote.',
          live: true,
        ),
        const SizedBox(height: 10),
        _ListenGhostCta(
          accent: accent,
          icon: Icons.close_rounded,
          label: 'Cancel',
          onTap: onCancel,
        ),
      ],
    );
  }
}

/// The server forwarded a press — surface the candidate and ask the user to
/// confirm. Tries-again drops back to listening, Use-this commits.
class _ListenStateDetected extends StatelessWidget {
  final Color accent;
  final _MockButtonDevice? device;
  final VoidCallback onRetry;
  final VoidCallback onConfirm;

  const _ListenStateDetected({
    super.key,
    required this.accent,
    required this.device,
    required this.onRetry,
    required this.onConfirm,
  });

  @override
  Widget build(BuildContext context) {
    return Column(
      key: const ValueKey('detected-col'),
      mainAxisSize: MainAxisSize.min,
      children: [
        Padding(
          padding: const EdgeInsets.only(bottom: 10),
          child: Text(
            'Got one!',
            style: TextStyle(
              color: accent.withValues(alpha: 0.95),
              fontSize: 13,
              fontWeight: FontWeight.w800,
              letterSpacing: 1.2,
            ),
          ),
        ),
        _DetectedDeviceCard(accent: accent, device: device),
        const SizedBox(height: 10),
        Row(
          children: [
            Expanded(
              child: _ListenGhostCta(
                accent: accent,
                icon: Icons.refresh_rounded,
                label: 'Try again',
                onTap: onRetry,
              ),
            ),
            const SizedBox(width: 8),
            Expanded(
              child: _ListenFilledCta(
                accent: accent,
                icon: Icons.check_rounded,
                label: 'Use this button',
                onTap: onConfirm,
              ),
            ),
          ],
        ),
      ],
    );
  }
}

/// Bound state — a button is committed. A small chip displays it; tapping
/// the secondary CTA enters listen mode again to rebind.
class _ListenStateBound extends StatelessWidget {
  final Color accent;
  final _MockButtonDevice device;
  final VoidCallback onListen;
  final VoidCallback onClear;

  const _ListenStateBound({
    super.key,
    required this.accent,
    required this.device,
    required this.onListen,
    required this.onClear,
  });

  @override
  Widget build(BuildContext context) {
    return Column(
      key: const ValueKey('bound-col'),
      mainAxisSize: MainAxisSize.min,
      children: [
        Container(
          padding: const EdgeInsets.fromLTRB(12, 10, 14, 10),
          decoration: BoxDecoration(
            color: accent.withValues(alpha: 0.05),
            borderRadius: BorderRadius.circular(14),
            border: Border.all(color: accent.withValues(alpha: 0.15)),
          ),
          child: Row(
            children: [
              Container(
                width: 30,
                height: 30,
                alignment: Alignment.center,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: accent.withValues(alpha: 0.10),
                  border: Border.all(color: accent.withValues(alpha: 0.30)),
                ),
                child: Icon(
                  Icons.check_rounded,
                  size: 14,
                  color: accent.withValues(alpha: 0.85),
                ),
              ),
              const SizedBox(width: 10),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(
                      device.name,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        color: CelestialColors.textPrimary
                            .withValues(alpha: 0.92),
                        fontSize: 13,
                        fontWeight: FontWeight.w700,
                        letterSpacing: 0.1,
                      ),
                    ),
                    const SizedBox(height: 1),
                    Text(
                      device.location ?? 'Bound',
                      style: TextStyle(
                        color: CelestialColors.textSecondary
                            .withValues(alpha: 0.55),
                        fontSize: 11,
                        fontWeight: FontWeight.w500,
                      ),
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
        const SizedBox(height: 8),
        Row(
          children: [
            Expanded(
              child: GestureDetector(
                onTap: onClear,
                behavior: HitTestBehavior.opaque,
                child: Container(
                  padding:
                      const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
                  decoration: BoxDecoration(
                    borderRadius: BorderRadius.circular(10),
                    color: accent.withValues(alpha: 0.04),
                    border: Border.all(
                      color: accent.withValues(alpha: 0.18),
                    ),
                  ),
                  child: Row(
                    mainAxisAlignment: MainAxisAlignment.center,
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Icon(
                        Icons.link_off_rounded,
                        size: 13,
                        color: accent.withValues(alpha: 0.6),
                      ),
                      const SizedBox(width: 5),
                      Text(
                        'Unbind',
                        style: TextStyle(
                          color: accent.withValues(alpha: 0.6),
                          fontSize: 11.5,
                          fontWeight: FontWeight.w600,
                          letterSpacing: 0.3,
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
            const SizedBox(width: 8),
            Expanded(
              child: GestureDetector(
                onTap: onListen,
                behavior: HitTestBehavior.opaque,
                child: Container(
                  padding:
                      const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
                  decoration: BoxDecoration(
                    borderRadius: BorderRadius.circular(10),
                    color: accent.withValues(alpha: 0.06),
                    border:
                        Border.all(color: accent.withValues(alpha: 0.18)),
                  ),
                  child: Row(
                    mainAxisAlignment: MainAxisAlignment.center,
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Icon(
                        Icons.sensors_rounded,
                        size: 13,
                        color: accent.withValues(alpha: 0.7),
                      ),
                      const SizedBox(width: 5),
                      Text(
                        'Rebind',
                        style: TextStyle(
                          color: accent.withValues(alpha: 0.7),
                          fontSize: 11.5,
                          fontWeight: FontWeight.w700,
                          letterSpacing: 0.3,
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ],
        ),
      ],
    );
  }
}

/// The detected-button card slot. Falls back to a placeholder if the
/// payload didn't carry name/location yet.
class _DetectedDeviceCard extends StatelessWidget {
  final Color accent;
  final _MockButtonDevice? device;

  const _DetectedDeviceCard({required this.accent, required this.device});

  @override
  Widget build(BuildContext context) {
    final name = device?.name ?? 'Detected button';
    final location = device?.location;
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.fromLTRB(14, 12, 14, 12),
      decoration: BoxDecoration(
        color: accent.withValues(alpha: 0.07),
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: accent.withValues(alpha: 0.28)),
        boxShadow: [
          BoxShadow(
            color: accent.withValues(alpha: 0.10),
            blurRadius: 14,
            spreadRadius: -3,
          ),
        ],
      ),
      child: Row(
        children: [
          Container(
            width: 32,
            height: 32,
            alignment: Alignment.center,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: accent.withValues(alpha: 0.16),
              border: Border.all(color: accent.withValues(alpha: 0.45)),
            ),
            child: Icon(
              Icons.radio_button_checked_rounded,
              size: 14,
              color: accent.withValues(alpha: 0.95),
            ),
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(
                  name,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: CelestialColors.textPrimary.withValues(alpha: 0.96),
                    fontSize: 13.5,
                    fontWeight: FontWeight.w700,
                    letterSpacing: 0.1,
                  ),
                ),
                const SizedBox(height: 2),
                Text(
                  location ?? 'Forwarded from your hub',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: CelestialColors.textSecondary.withValues(alpha: 0.7),
                    fontSize: 11,
                    fontWeight: FontWeight.w500,
                    letterSpacing: 0.2,
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

/// Two-line caption block used under the radar stage. A small "LIVE" dot
/// pulses next to the title while the server is forwarding events.
class _ListenCaption extends StatelessWidget {
  final Color accent;
  final String title;
  final String subtitle;
  final bool live;

  const _ListenCaption({
    required this.accent,
    required this.title,
    required this.subtitle,
    this.live = false,
  });

  @override
  Widget build(BuildContext context) {
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            if (live) ...[
              _BlinkingDot(color: accent),
              const SizedBox(width: 8),
            ],
            Text(
              title,
              style: TextStyle(
                color: CelestialColors.textPrimary.withValues(alpha: 0.92),
                fontSize: 13.5,
                fontWeight: FontWeight.w700,
                letterSpacing: 0.2,
              ),
            ),
          ],
        ),
        const SizedBox(height: 4),
        Text(
          subtitle,
          textAlign: TextAlign.center,
          style: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.65),
            fontSize: 11.5,
            fontWeight: FontWeight.w500,
            letterSpacing: 0.2,
          ),
        ),
      ],
    );
  }
}

class _BlinkingDot extends StatefulWidget {
  final Color color;

  const _BlinkingDot({required this.color});

  @override
  State<_BlinkingDot> createState() => _BlinkingDotState();
}

class _BlinkingDotState extends State<_BlinkingDot>
    with SingleTickerProviderStateMixin {
  late final AnimationController _ctrl;

  @override
  void initState() {
    super.initState();
    _ctrl = AnimationController(
      duration: const Duration(milliseconds: 900),
      vsync: this,
    )..repeat(reverse: true);
  }

  @override
  void dispose() {
    _ctrl.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: _ctrl,
      builder: (context, _) {
        final t = Curves.easeInOut.transform(_ctrl.value);
        return Container(
          width: 7,
          height: 7,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: widget.color.withValues(alpha: 0.40 + 0.55 * t),
            boxShadow: [
              BoxShadow(
                color: widget.color.withValues(alpha: 0.35 * t),
                blurRadius: 8,
                spreadRadius: -1,
              ),
            ],
          ),
        );
      },
    );
  }
}

/// Primary action — filled accent border + label. The CTA that initiates the
/// Listen flow when no button is bound.
class _ListenPrimaryCta extends StatelessWidget {
  final Color accent;
  final IconData icon;
  final String label;
  final VoidCallback onTap;

  const _ListenPrimaryCta({
    required this.accent,
    required this.icon,
    required this.label,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: onTap,
      behavior: HitTestBehavior.opaque,
      child: Container(
        width: double.infinity,
        padding: const EdgeInsets.symmetric(vertical: 12, horizontal: 18),
        decoration: BoxDecoration(
          color: accent.withValues(alpha: 0.10),
          borderRadius: BorderRadius.circular(14),
          border: Border.all(color: accent.withValues(alpha: 0.40), width: 1),
          boxShadow: [
            BoxShadow(
              color: accent.withValues(alpha: 0.16),
              blurRadius: 14,
              spreadRadius: -3,
            ),
          ],
        ),
        child: Row(
          mainAxisAlignment: MainAxisAlignment.center,
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(icon, size: 16, color: accent.withValues(alpha: 0.95)),
            const SizedBox(width: 8),
            Text(
              label,
              style: TextStyle(
                color: accent.withValues(alpha: 0.95),
                fontSize: 13,
                fontWeight: FontWeight.w800,
                letterSpacing: 0.6,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// Quieter outline CTA — used for Cancel and "Listen for a different button".
class _ListenGhostCta extends StatelessWidget {
  final Color accent;
  final IconData icon;
  final String label;
  final VoidCallback onTap;

  const _ListenGhostCta({
    required this.accent,
    required this.icon,
    required this.label,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: onTap,
      behavior: HitTestBehavior.opaque,
      child: Container(
        width: double.infinity,
        padding: const EdgeInsets.symmetric(vertical: 11, horizontal: 14),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(14),
          border: Border.all(color: accent.withValues(alpha: 0.22)),
        ),
        child: Row(
          mainAxisAlignment: MainAxisAlignment.center,
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(icon, size: 14, color: accent.withValues(alpha: 0.82)),
            const SizedBox(width: 8),
            Flexible(
              child: Text(
                label,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  color: accent.withValues(alpha: 0.85),
                  fontSize: 12,
                  fontWeight: FontWeight.w700,
                  letterSpacing: 0.4,
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// Filled accent CTA — used for Use-this-button to read as the decisive
/// action in the confirm row.
class _ListenFilledCta extends StatelessWidget {
  final Color accent;
  final IconData icon;
  final String label;
  final VoidCallback onTap;

  const _ListenFilledCta({
    required this.accent,
    required this.icon,
    required this.label,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: onTap,
      behavior: HitTestBehavior.opaque,
      child: Container(
        width: double.infinity,
        padding: const EdgeInsets.symmetric(vertical: 11, horizontal: 14),
        decoration: BoxDecoration(
          color: accent.withValues(alpha: 0.94),
          borderRadius: BorderRadius.circular(14),
          boxShadow: [
            BoxShadow(
              color: accent.withValues(alpha: 0.30),
              blurRadius: 14,
              spreadRadius: -3,
            ),
          ],
        ),
        child: Row(
          mainAxisAlignment: MainAxisAlignment.center,
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(icon, size: 14, color: CelestialColors.backgroundDark),
            const SizedBox(width: 8),
            Flexible(
              child: Text(
                label,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: const TextStyle(
                  color: CelestialColors.backgroundDark,
                  fontSize: 12,
                  fontWeight: FontWeight.w800,
                  letterSpacing: 0.5,
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// Visual stage at the top of the listen flow. Three modes:
/// - dormant: a static halo around the button glyph (idle, unbound)
/// - scanning: three concentric rings expanding outward and fading
/// - locked: a tight ring with a crosshair, indicating a captured event
enum _RadarStageMode { dormant, scanning, locked }

class _RadarStage extends StatelessWidget {
  final Color accent;
  final Animation<double>? pulse;
  final _RadarStageMode mode;
  final double diameter;

  const _RadarStage({
    required this.accent,
    required this.pulse,
    required this.mode,
    required this.diameter,
  });

  @override
  Widget build(BuildContext context) {
    final repaint = pulse;
    return SizedBox.square(
      dimension: diameter,
      child: CustomPaint(
        painter: _RadarPainter(
          accent: accent,
          mode: mode,
          progress: pulse?.value ?? 0.0,
          repaint: repaint,
        ),
        child: Center(
          child: _RadarCore(accent: accent, mode: mode, pulse: pulse),
        ),
      ),
    );
  }
}

class _RadarCore extends StatelessWidget {
  final Color accent;
  final _RadarStageMode mode;
  // Provided while listening so the target itself breathes in sync with the
  // outgoing rings. Null in dormant/locked states keeps it perfectly still.
  final Animation<double>? pulse;

  const _RadarCore({
    required this.accent,
    required this.mode,
    required this.pulse,
  });

  @override
  Widget build(BuildContext context) {
    final activePulse = mode == _RadarStageMode.scanning ? pulse : null;
    if (activePulse == null) {
      return _buildCore(scale: 1.0, glowAlpha: 0.28, fillAlpha: 0.12);
    }
    return AnimatedBuilder(
      animation: activePulse,
      builder: (context, _) {
        // Single-pulse-per-cycle breath, peaking mid-cycle so the target
        // visibly "ticks" with each ring emission.
        final wave = math.sin(activePulse.value * math.pi);
        return _buildCore(
          scale: 1.0 + wave * 0.07,
          glowAlpha: 0.28 + wave * 0.20,
          fillAlpha: 0.12 + wave * 0.08,
        );
      },
    );
  }

  Widget _buildCore({
    required double scale,
    required double glowAlpha,
    required double fillAlpha,
  }) {
    final size = mode == _RadarStageMode.locked ? 48.0 : 52.0;
    final icon = mode == _RadarStageMode.locked
        ? Icons.check_rounded
        : Icons.radio_button_checked_rounded;
    return Transform.scale(
      scale: scale,
      child: Container(
        width: size,
        height: size,
        alignment: Alignment.center,
        decoration: BoxDecoration(
          shape: BoxShape.circle,
          color: accent.withValues(alpha: fillAlpha),
          border: Border.all(color: accent.withValues(alpha: 0.55), width: 1.4),
          boxShadow: [
            BoxShadow(
              color: accent.withValues(alpha: glowAlpha),
              blurRadius: 18,
              spreadRadius: -2,
            ),
          ],
        ),
        child: Icon(
          icon,
          size: size * 0.46,
          color: accent.withValues(alpha: 0.95),
        ),
      ),
    );
  }
}

class _RadarPainter extends CustomPainter {
  final Color accent;
  final _RadarStageMode mode;
  final double progress;

  _RadarPainter({
    required this.accent,
    required this.mode,
    required this.progress,
    super.repaint,
  });

  @override
  void paint(Canvas canvas, Size size) {
    final center = Offset(size.width / 2, size.height / 2);
    final maxR = size.shortestSide / 2;

    // Always-present dim grid ring — anchors the stage when nothing else is
    // animating.
    canvas.drawCircle(
      center,
      maxR * 0.95,
      Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = 0.8
        ..color = accent.withValues(alpha: 0.06),
    );

    switch (mode) {
      case _RadarStageMode.dormant:
        _paintDormant(canvas, center, maxR);
        break;
      case _RadarStageMode.scanning:
        _paintScanning(canvas, center, maxR);
        break;
      case _RadarStageMode.locked:
        _paintLocked(canvas, center, maxR);
        break;
    }
  }

  void _paintDormant(Canvas canvas, Offset center, double maxR) {
    // Two faint static rings inviting the user to "tune in".
    for (final t in [0.55, 0.78]) {
      canvas.drawCircle(
        center,
        maxR * t,
        Paint()
          ..style = PaintingStyle.stroke
          ..strokeWidth = 0.8
          ..color = accent.withValues(alpha: 0.10),
      );
    }
  }

  void _paintScanning(Canvas canvas, Offset center, double maxR) {
    // Three rings offset in phase so they ripple outward continuously.
    for (int i = 0; i < 3; i++) {
      final phase = (progress + i / 3.0) % 1.0;
      final r = maxR * (0.18 + phase * 0.82);
      final fade = 1.0 - phase;
      final alpha = (fade * fade) * 0.55;
      canvas.drawCircle(
        center,
        r,
        Paint()
          ..style = PaintingStyle.stroke
          ..strokeWidth = 1.2
          ..color = accent.withValues(alpha: alpha),
      );
    }
  }

  void _paintLocked(Canvas canvas, Offset center, double maxR) {
    // A single tight ring "snaps" around the core, with four crosshair
    // ticks reading as a target lock.
    final ringR = maxR * 0.62;
    canvas.drawCircle(
      center,
      ringR,
      Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = 1.4
        ..color = accent.withValues(alpha: 0.55),
    );
    canvas.drawCircle(
      center,
      ringR + 4,
      Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = 4
        ..color = accent.withValues(alpha: 0.10)
        ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 4),
    );
    final tickPaint = Paint()
      ..strokeWidth = 1.4
      ..strokeCap = StrokeCap.round
      ..color = accent.withValues(alpha: 0.65);
    const tickIn = 4.0;
    const tickOut = 10.0;
    for (int i = 0; i < 4; i++) {
      final angle = i * math.pi / 2;
      final cos = math.cos(angle);
      final sin = math.sin(angle);
      canvas.drawLine(
        Offset(center.dx + cos * (ringR + tickIn),
            center.dy + sin * (ringR + tickIn)),
        Offset(center.dx + cos * (ringR + tickOut),
            center.dy + sin * (ringR + tickOut)),
        tickPaint,
      );
    }
  }

  @override
  bool shouldRepaint(covariant _RadarPainter old) =>
      old.progress != progress || old.mode != mode || old.accent != accent;
}

/// Quiet outline pill — left half of the inline save/reset cluster in the
/// Time section. Reverts in-flight edits back to the last saved snapshot.
class _ResetPill extends StatelessWidget {
  final Color accentColor;
  final VoidCallback? onTap;

  const _ResetPill({
    required this.accentColor,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    final enabled = onTap != null;
    final alpha = enabled ? 1.0 : 0.45;
    return GestureDetector(
      onTap: onTap,
      behavior: HitTestBehavior.opaque,
      child: Container(
        padding: const EdgeInsets.fromLTRB(11, 7, 13, 7),
        decoration: BoxDecoration(
          color: Colors.transparent,
          borderRadius: BorderRadius.circular(20),
          border: Border.all(
            color: accentColor.withValues(alpha: 0.22 * alpha),
            width: 1,
          ),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(
              Icons.restart_alt_rounded,
              size: 13,
              color: accentColor.withValues(alpha: 0.85 * alpha),
            ),
            const SizedBox(width: 5),
            Text(
              'Revert',
              style: TextStyle(
                color: accentColor.withValues(alpha: 0.85 * alpha),
                fontSize: 11,
                fontWeight: FontWeight.w700,
                letterSpacing: 0.5,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// Filled companion pill — right half of the inline save/reset cluster.
/// Commits every unsaved change on the page. Disabled tap (null `onTap`)
/// renders muted; `isSaving` swaps the icon for a spinner.
class _SavePill extends StatelessWidget {
  final Color accentColor;
  final bool isSaving;
  final bool synced;
  final VoidCallback? onTap;

  const _SavePill({
    required this.accentColor,
    required this.isSaving,
    required this.synced,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    final enabled = onTap != null && synced && !isSaving;
    final label = isSaving
        ? 'Saving'
        : !synced
            ? 'Offline'
            : 'Save';
    final alpha = enabled ? 1.0 : 0.55;
    return GestureDetector(
      onTap: onTap,
      behavior: HitTestBehavior.opaque,
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 180),
        padding: const EdgeInsets.fromLTRB(11, 7, 13, 7),
        decoration: BoxDecoration(
          color: enabled
              ? accentColor.withValues(alpha: 0.16)
              : accentColor.withValues(alpha: 0.05),
          borderRadius: BorderRadius.circular(20),
          border: Border.all(
            color: enabled
                ? accentColor.withValues(alpha: 0.55)
                : accentColor.withValues(alpha: 0.18),
            width: 1,
          ),
          boxShadow: [
            if (enabled)
              BoxShadow(
                color: accentColor.withValues(alpha: 0.18),
                blurRadius: 10,
                spreadRadius: -2,
              ),
          ],
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            if (isSaving)
              SizedBox(
                width: 11,
                height: 11,
                child: CircularProgressIndicator(
                  strokeWidth: 1.6,
                  valueColor: AlwaysStoppedAnimation<Color>(
                    accentColor.withValues(alpha: 0.9),
                  ),
                ),
              )
            else
              Icon(
                synced ? Icons.check_rounded : Icons.cloud_off_rounded,
                size: 13,
                color: accentColor.withValues(alpha: 0.95 * alpha),
              ),
            const SizedBox(width: 5),
            Text(
              label,
              style: TextStyle(
                color: accentColor.withValues(alpha: 0.95 * alpha),
                fontSize: 11,
                fontWeight: FontWeight.w700,
                letterSpacing: 0.5,
              ),
            ),
          ],
        ),
      ),
    );
  }
}


String _formatDuration(int ms) {
  final seconds = ms ~/ 1000;
  if (seconds < 60) return '${seconds}s';
  final minutes = seconds ~/ 60;
  final remainingSeconds = seconds % 60;
  if (remainingSeconds == 0) return '$minutes min';
  return '${minutes}m ${remainingSeconds}s';
}

String _shortAnchorLabel(TriggerAnchor anchor) {
  return anchor.label;
}

String _shortTriggerLabel(RhythmTransitionTrigger trigger) {
  if (trigger.isScheduled) return '';
  if (trigger.kind == 'manual') return '';
  return switch (trigger.event) {
    'sunrise' => 'Sunrise',
    'civil_twilight' => 'Civil Twilight',
    'nautical_twilight' => 'Nautical Twilight',
    'astronomical_twilight' => 'Astronomical Twilight',
    'sunset' => 'Sunset',
    _ => trigger.event?.replaceAll('_', ' ') ?? '?',
  };
}

Color _fallbackModeColor(RhythmMode mode) => switch (mode) {
      RhythmMode.day => const Color(0xFFF9A825),
      RhythmMode.sleep => const Color(0xFFE57373),
    };

IconData _modeIcon(RhythmMode mode) => switch (mode) {
      RhythmMode.day => Icons.wb_sunny_rounded,
      RhythmMode.sleep => Icons.bedtime_rounded,
    };

String _modeLabel(RhythmMode mode) => switch (mode) {
      RhythmMode.day => 'Day',
      RhythmMode.sleep => 'Sleep',
    };

String _colorSignature(Color color) {
  return '${(color.a * 255).round()},'
      '${(color.r * 255).round()},'
      '${(color.g * 255).round()},'
      '${(color.b * 255).round()}';
}

String _formatScheduledTriggerTime(double hour) {
  final totalMinutes =
      (SolarUtils.normalizeHour(hour) * 60).round().clamp(0, 1439);
  final hours = totalMinutes ~/ 60;
  final minutes = totalMinutes % 60;
  return '${hours.toString().padLeft(2, '0')}:${minutes.toString().padLeft(2, '0')}';
}

double? _scheduledTimeToHour(String time) {
  final parts = time.split(':');
  if (parts.length != 2) return null;
  final hours = int.tryParse(parts[0]);
  final minutes = int.tryParse(parts[1]);
  if (hours == null || minutes == null) return null;
  if (hours < 0 || hours > 23 || minutes < 0 || minutes > 59) return null;
  return hours + minutes / 60.0;
}
