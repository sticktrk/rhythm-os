import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart' hide Home, Hub, HubType;
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../../api/hybrid_client.dart' show sdkCurveConfigToDto;
import '../../models/plan_tier.dart';
import '../../providers/home_provider.dart';
import '../../providers/server_sync_provider.dart';
import '../../providers/subscription_provider.dart';
import '../../widgets/info_tooltip.dart';
import '../../widgets/plan_tier_modal.dart';
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
  static const double _eventSnapThresholdHours = 0.25;
  static const double _minimumHandleGapHours = 0.25;
  static const double _maxEditableHour = 23.99;
  // Piecewise duration scale. Below the bend, the slider steps 1s at a time
  // so short transitions (think a quick room cue) are tunable; above the bend
  // it falls back to 10s steps so the long end stays reachable without 270
  // ticks across the track.
  static const double _minDurationSeconds = 1.0;
  static const double _bendDurationSeconds = 30.0;
  static const double _maxDurationSeconds = 300.0;
  static const double _fineDurationStepSeconds = 1.0;
  static const double _coarseDurationStepSeconds = 10.0;
  // Angular drag on a small orbit translates pixels to many minutes per
  // degree, so the orb feels twitchy. Quantizing to 5-minute "stops" gives a
  // sundial-style ratchet without losing precision worth caring about.
  static const double _dragStepHours = 5.0 / 60.0;

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
  late AnimationController _breatheController;
  late Animation<double> _breatheAnimation;
  late AnimationController _flowController;
  late Animation<double> _flowAnimation;
  final Map<RhythmMode, double> _handleHours = {};
  final Map<RhythmMode, _ModeCurveVisual> _curveVisuals = {};
  late final ServerSyncProvider _serverSync;
  late final HomeProvider _homeProvider;
  String? _profileVisualSignature;
  SolarClockData? _solarClockData;
  String? _solarLocationKey;
  RhythmMode? _dragMode;
  double? _dragPreviewHour;
  _TriggerAnchor? _proximateAnchor;
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
    _breatheController = AnimationController(
      duration: const Duration(milliseconds: 3500),
      vsync: this,
    )..repeat(reverse: true);
    _breatheAnimation = Tween<double>(
      begin: 0.0,
      end: 1.0,
    ).animate(
      CurvedAnimation(parent: _breatheController, curve: Curves.easeInOut),
    );
    _flowController = AnimationController(
      duration: const Duration(milliseconds: 2800),
      vsync: this,
    )..repeat();
    _flowAnimation = CurvedAnimation(
      parent: _flowController,
      curve: Curves.easeInOut,
    );
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
    _breatheController.dispose();
    _flowController.dispose();
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
    try {
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
      final now = DateTime.now();
      final sunTimes = getSunTimes(
        latitude: loc.latitude,
        longitude: loc.longitude,
        year: now.year,
        month: now.month,
        day: now.day,
        timezone: tz,
      );
      final twilightTimes = getTwilightTimes(
        latitude: loc.latitude,
        longitude: loc.longitude,
        year: now.year,
        month: now.month,
        day: now.day,
        timezone: tz,
      );
      _solarClockData = SolarClockData(
        sunTimes: sunTimes,
        twilightTimes: twilightTimes,
      );
      _rebuildCurveVisuals(
        latitude: loc.latitude,
        longitude: loc.longitude,
        year: now.year,
        month: now.month,
        day: now.day,
        timezone: tz,
      );
    } catch (_) {
      _solarClockData = null;
      _curveVisuals.clear();
      _profileVisualSignature = _currentProfileVisualSignature();
    }
  }

  void _rebuildCurveVisuals({
    double? latitude,
    double? longitude,
    int? year,
    int? month,
    int? day,
    String? timezone,
  }) {
    final home = context.read<HomeProvider>().currentHome;
    final loc = home?.location;
    if (loc == null) {
      _curveVisuals.clear();
      _profileVisualSignature = _currentProfileVisualSignature();
      return;
    }

    final now = DateTime.now();
    _curveVisuals
      ..clear()
      ..addAll(
        _buildModeCurveVisuals(
          latitude: latitude ?? loc.latitude,
          longitude: longitude ?? loc.longitude,
          year: year ?? now.year,
          month: month ?? now.month,
          day: day ?? now.day,
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

  Map<RhythmMode, _ModeCurveVisual> _buildModeCurveVisuals({
    required double latitude,
    required double longitude,
    required int year,
    required int month,
    required int day,
    required String timezone,
  }) {
    return {
      RhythmMode.day: _buildModeCurveVisual(
        mode: RhythmMode.day,
        latitude: latitude,
        longitude: longitude,
        year: year,
        month: month,
        day: day,
        timezone: timezone,
      ),
      RhythmMode.sleep: _buildModeCurveVisual(
        mode: RhythmMode.sleep,
        latitude: latitude,
        longitude: longitude,
        year: year,
        month: month,
        day: day,
        timezone: timezone,
      ),
    };
  }

  _ModeCurveVisual _buildModeCurveVisual({
    required RhythmMode mode,
    required double latitude,
    required double longitude,
    required int year,
    required int month,
    required int day,
    required String timezone,
  }) {
    final fallbackColor =
        widget.profileColors[mode] ?? _fallbackModeColor(mode);
    final profile = _activeProfileForMode(mode);
    if (profile == null) {
      return _ModeCurveVisual(
        fallbackColor: fallbackColor,
        fallbackBrightness: 50,
      );
    }

    final curve = profile.curve;
    final fallbackBrightness = switch (curve) {
      RhythmConstantCurve() => curve.brightness,
      _ => ((profile.minBrightness + profile.maxBrightness) / 2).round(),
    };

    if (curve is! RhythmSuperGaussianCurve) {
      return _ModeCurveVisual(
        fallbackColor: fallbackColor,
        fallbackBrightness: fallbackBrightness,
      );
    }

    try {
      final curveData = generateCurveDataWithSunTimes(
        config: sdkCurveConfigToDto(profile),
        latitude: latitude,
        longitude: longitude,
        year: year,
        month: month,
        day: day,
        timezone: timezone,
      );
      return _ModeCurveVisual(
        samples: SolarCurveSamples.fromCurveDataDto(curveData),
        fallbackColor: fallbackColor,
        fallbackBrightness: fallbackBrightness,
      );
    } catch (_) {
      return _ModeCurveVisual(
        fallbackColor: fallbackColor,
        fallbackBrightness: fallbackBrightness,
      );
    }
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
    _TriggerAnchor? snappedAnchor,
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
        AutomationDetail.automatic => 'Day/Sleep Automatic',
        AutomationDetail.button => 'Day/Sleep Button Toggle',
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

    return AnimatedBuilder(
      animation: Listenable.merge([_flowAnimation, _breatheAnimation]),
      builder: (context, _) {
        return SizedBox.expand(
          child: SolarClock(
            data: solarClockData,
            use24: MediaQuery.alwaysUse24HourFormatOf(context),
            showUpperArc: false,
            showEventMarkers: false,
            showHourLabels: false,
            showLowerArc: false,
            horizonFactor: 0.58,
            radiusWidthFactor: 0.35,
            radiusHeightFactor: 0.92,
            underlayBuilder: (context, geometry) => _buildClockRing(
              geometry: geometry,
              dayColor: dayColor,
              sleepColor: sleepColor,
              use24: MediaQuery.alwaysUse24HourFormatOf(context),
            ),
            overlayBuilder: (context, geometry) => _buildClockOverlay(
              geometry: geometry,
              dayColor: dayColor,
              sleepColor: sleepColor,
            ),
          ),
        );
      },
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

    return Container(
      padding: const EdgeInsets.fromLTRB(12, 8, 12, 10),
      decoration: BoxDecoration(
        color: modeColor.withValues(alpha: 0.08),
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: modeColor.withValues(alpha: 0.18)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          Row(
            children: [
              Icon(_modeIcon(mode), size: 14, color: modeColor),
              const SizedBox(width: 6),
              Flexible(
                child: Text(
                  '${_modeLabel(mode)} Start',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    color: modeColor,
                    fontSize: 12,
                    fontWeight: FontWeight.w700,
                    letterSpacing: 0.4,
                  ),
                ),
              ),
            ],
          ),
          const SizedBox(height: 4),
          Text(
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
        ],
      ),
    );
  }

  Widget _buildClockRing({
    required SolarClockGeometry geometry,
    required Color dayColor,
    required Color sleepColor,
    required bool use24,
  }) {
    final dayHour = _displayHourForMode(
      RhythmMode.day,
      fallback: _handleHourForMode(RhythmMode.day) ?? 6.0,
    );
    final sleepHour = _displayHourForMode(
      RhythmMode.sleep,
      fallback: _handleHourForMode(RhythmMode.sleep) ?? 22.0,
    );

    return CustomPaint(
      painter: _RhythmClockRingPainter(
        geometry: geometry,
        dayStartHour: dayHour,
        sleepStartHour: sleepHour,
        dayVisual: _curveVisuals[RhythmMode.day] ??
            _ModeCurveVisual(fallbackColor: dayColor),
        sleepVisual: _curveVisuals[RhythmMode.sleep] ??
            _ModeCurveVisual(fallbackColor: sleepColor),
        selectedMode: _dragMode ?? _selectedMode,
        use24: use24,
      ),
    );
  }

  Widget _buildClockOverlay({
    required SolarClockGeometry geometry,
    required Color dayColor,
    required Color sleepColor,
  }) {
    final handles = _handleSpecs(dayColor: dayColor, sleepColor: sleepColor);
    final handleByMode = {for (final handle in handles) handle.mode: handle};

    // Use a `RawGestureDetector` with an overriding pan recognizer so the
    // orbit drag always wins the gesture arena. Without this, once the
    // page's scroll view starts competing for vertical pans (e.g. when the
    // save/revert cluster expands content past the viewport), the orbs
    // become un-draggable.
    return RawGestureDetector(
      behavior: HitTestBehavior.translucent,
      gestures: <Type, GestureRecognizerFactory>{
        _ClockPanGestureRecognizer:
            GestureRecognizerFactoryWithHandlers<_ClockPanGestureRecognizer>(
          () => _ClockPanGestureRecognizer(),
          (recognizer) {
            recognizer.onStart = (details) {
              _handleClockPanStart(
                  details.localPosition, geometry, handleByMode);
            };
            recognizer.onUpdate = (details) {
              _handleClockPanUpdate(details.localPosition, geometry);
            };
            recognizer.onEnd = (_) {
              _handleClockPanEnd();
            };
            recognizer.onCancel = _handleClockPanEnd;
          },
        ),
        TapGestureRecognizer:
            GestureRecognizerFactoryWithHandlers<TapGestureRecognizer>(
          () => TapGestureRecognizer(),
          (recognizer) {
            recognizer.onTapUp = (details) {
              _handleClockTap(details.localPosition, geometry, handleByMode);
            };
          },
        ),
      },
      child: Stack(
        clipBehavior: Clip.none,
        children: [
          Positioned.fill(
            child: CustomPaint(
              painter: _ClockHandlesPainter(
                geometry: geometry,
                handles: handles,
                selectedMode: _selectedMode,
                flowProgress: _flowAnimation.value,
              ),
            ),
          ),
          ..._buildTwilightAnchorIndicators(
            geometry: geometry,
            dayColor: dayColor,
            sleepColor: sleepColor,
          ),
          for (final handle in handles)
            _buildPositionedHandle(handle: handle, geometry: geometry),
        ],
      ),
    );
  }

  Widget _buildPositionedHandle({
    required _TransitionHandleSpec handle,
    required SolarClockGeometry geometry,
  }) {
    final isSelected = handle.mode == _selectedMode;
    final center = geometry.positionForHour(handle.hour);
    final width = isSelected ? 70.0 : 62.0;
    final orbRadius = (isSelected ? 44.0 : 40.0) / 2;

    return Positioned(
      left: center.dx - width / 2,
      top: center.dy - orbRadius,
      child: _ClockModeOrb(
        mode: handle.mode,
        color: handle.color,
        timeLabel: _fmtTime(handle.hour),
        selected: isSelected,
      ),
    );
  }

  List<Widget> _buildTwilightAnchorIndicators({
    required SolarClockGeometry geometry,
    required Color dayColor,
    required Color sleepColor,
  }) {
    final mode = _dragMode ?? _selectedMode;
    final isDragging = _dragMode != null;
    final modeColor = mode == RhythmMode.day ? dayColor : sleepColor;
    final isDawn = mode == RhythmMode.day;
    final baseColor =
        isDawn ? SolarClockData.dawnColor : SolarClockData.duskColor;
    final anchors = _triggerAnchorsForMode(mode);
    final handleHour =
        _displayHourForMode(mode, fallback: _handleHourForMode(mode) ?? 0.0);
    final activeSolarEvent =
        _config.trigger.isSolar ? _config.trigger.event : null;

    final widgets = <Widget>[
      Positioned.fill(
        child: IgnorePointer(
          child: CustomPaint(
            painter: _ClockSolarAnchorPainter(
              geometry: geometry,
              anchors: anchors,
              accentColor: Color.lerp(baseColor, modeColor, 0.45)!,
              activeEvent: activeSolarEvent,
              proximateAnchor: _proximateAnchor,
              anchorProximity: _anchorProximity,
              handleHour: handleHour,
              handleWidth: mode == _selectedMode ? 70.0 : 62.0,
              dragHour: _dragPreviewHour,
              pulse: _breatheAnimation.value,
              isDragging: isDragging,
            ),
          ),
        ),
      ),
    ];

    for (final anchor in anchors) {
      final isProximate = isDragging && _proximateAnchor?.event == anchor.event;
      final isActive = !isDragging && activeSolarEvent == anchor.event;
      final proximity = isProximate ? _anchorProximity : 0.0;

      if (!isDragging && !isActive) continue;

      final pos = geometry.positionForHour(anchor.hour);

      // Dot sizing: active pulses gently, dragging scales with proximity
      final breathe = _breatheAnimation.value;
      final dotSize = isActive ? 7.0 + breathe * 1.5 : 4.0 + proximity * 7.0;
      final dotAlpha = isActive ? 0.65 : 0.2 + proximity * 0.8;
      final glowRadius = isActive ? 6.0 + breathe * 3.0 : proximity * 16.0;
      final effectiveColor =
          isActive ? modeColor : Color.lerp(baseColor, modeColor, proximity)!;

      final totalSize = dotSize + glowRadius * 2;
      widgets.add(
        Positioned(
          left: pos.dx - totalSize / 2,
          top: pos.dy - totalSize / 2,
          child: IgnorePointer(
            child: SizedBox(
              width: totalSize,
              height: totalSize,
              child: Center(
                child: Container(
                  width: dotSize,
                  height: dotSize,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: effectiveColor.withValues(alpha: dotAlpha),
                    boxShadow: [
                      if (glowRadius > 0)
                        BoxShadow(
                          color: effectiveColor.withValues(
                            alpha: dotAlpha * 0.5,
                          ),
                          blurRadius: glowRadius,
                          spreadRadius: 1,
                        ),
                    ],
                  ),
                ),
              ),
            ),
          ),
        ),
      );
    }

    return widgets;
  }

  List<_TransitionHandleSpec> _handleSpecs({
    required Color dayColor,
    required Color sleepColor,
  }) {
    return [
      _handleSpecForMode(RhythmMode.day, dayColor),
      _handleSpecForMode(RhythmMode.sleep, sleepColor),
    ].whereType<_TransitionHandleSpec>().toList();
  }

  _TransitionHandleSpec? _handleSpecForMode(RhythmMode mode, Color color) {
    final hour = _handleHourForMode(mode);
    if (hour == null) return null;

    return _TransitionHandleSpec(
      mode: mode,
      hour: _displayHourForMode(mode, fallback: hour),
      color: color,
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

  void _handleClockTap(
    Offset position,
    SolarClockGeometry geometry,
    Map<RhythmMode, _TransitionHandleSpec> handleByMode,
  ) {
    final mode = _hitTestHandle(position, geometry, handleByMode);
    if (mode != null) _focusMode(mode);
  }

  void _handleClockPanStart(
    Offset position,
    SolarClockGeometry geometry,
    Map<RhythmMode, _TransitionHandleSpec> handleByMode,
  ) {
    final mode = _hitTestHandle(position, geometry, handleByMode);
    if (mode == null) return;
    HapticFeedback.lightImpact();
    final initialHour = _handleHourForMode(mode) ?? handleByMode[mode]?.hour;
    setState(() {
      _dragMode = mode;
      _selectedMode = mode;
      _dragPreviewHour = initialHour;
      if (initialHour != null) _computeDragProximity(mode, initialHour);
    });
  }

  void _handleClockPanUpdate(Offset position, SolarClockGeometry geometry) {
    final mode = _dragMode;
    if (mode == null) return;

    final rawHour = geometry.hourFromPosition(position);
    // Quantize the drag to 5-minute stops so small finger jitter doesn't
    // shift the time around.
    final steppedHour = (rawHour / _dragStepHours).round() * _dragStepHours;
    final draggedHour = _clampDraggedHour(mode, steppedHour);

    // Snap visually during drag so the orb locks onto solar events
    final snappedAnchor = _snappedAnchorForMode(mode, draggedHour);
    final displayHour = snappedAnchor?.hour ?? draggedHour;

    final previousProximate = _proximateAnchor;
    final previousDraggedHour = _handleHours[mode];
    final stepChanged = previousDraggedHour == null ||
        (draggedHour - previousDraggedHour).abs() > 0.0001;

    setState(() {
      _dragPreviewHour = displayHour;
      _handleHours[mode] = draggedHour;
      _computeDragProximity(mode, draggedHour);
    });

    if (stepChanged && snappedAnchor == null && previousDraggedHour != null) {
      HapticFeedback.selectionClick();
    }
    if (_proximateAnchor != null &&
        _proximateAnchor != previousProximate &&
        _anchorProximity > 0.75) {
      HapticFeedback.selectionClick();
    }
  }

  void _handleClockPanEnd() {
    final mode = _dragMode;
    if (mode == null) return;
    final finalHour = _dragPreviewHour ?? _handleHourForMode(mode);

    setState(() {
      _dragMode = null;
      _dragPreviewHour = null;
      _proximateAnchor = null;
      _anchorProximity = 0.0;
    });

    if (finalHour != null) {
      final snappedAnchor = _snappedAnchorForMode(mode, finalHour);
      _setModeHour(
        mode,
        snappedAnchor?.hour ?? finalHour,
        snappedAnchor: snappedAnchor,
      );
    }
  }

  RhythmMode? _hitTestHandle(
    Offset position,
    SolarClockGeometry geometry,
    Map<RhythmMode, _TransitionHandleSpec> handleByMode,
  ) {
    const threshold = 38.0;
    for (final entry in handleByMode.entries) {
      final center = geometry.positionForHour(entry.value.hour);
      if ((position - center).distance <= threshold) {
        return entry.key;
      }
    }
    return null;
  }

  double _displayHourForMode(RhythmMode mode, {required double fallback}) {
    if (_dragMode == mode && _dragPreviewHour != null) {
      return _dragPreviewHour!;
    }
    return _handleHourForMode(mode) ?? fallback;
  }

  double _clampDraggedHour(RhythmMode mode, double rawHour) {
    final normalized = SolarUtils.normalizeHour(rawHour);
    final dayHour = _handleHourForMode(RhythmMode.day) ?? 6.0;
    final sleepHour = _handleHourForMode(RhythmMode.sleep) ?? 22.0;

    if (mode == RhythmMode.day) {
      final maxDayHour = math.max(0.0, sleepHour - _minimumHandleGapHours);
      return normalized.clamp(0.0, maxDayHour).toDouble();
    }

    final minSleepHour =
        math.min(_maxEditableHour, dayHour + _minimumHandleGapHours);
    return normalized.clamp(minSleepHour, _maxEditableHour).toDouble();
  }

  void _computeDragProximity(RhythmMode mode, double dragHour) {
    final anchors = _triggerAnchorsForMode(mode)
        .where((a) => _isHourAllowedForMode(mode, a.hour))
        .toList();
    if (anchors.isEmpty) {
      _proximateAnchor = null;
      _anchorProximity = 0.0;
      return;
    }
    _TriggerAnchor? nearest;
    var nearestDist = double.infinity;
    for (final a in anchors) {
      final dist = (dragHour - a.hour).abs();
      if (dist < nearestDist) {
        nearestDist = dist;
        nearest = a;
      }
    }
    const awarenessRadius = 1.5;
    if (nearest != null && nearestDist <= awarenessRadius) {
      _proximateAnchor = nearest;
      _anchorProximity = (1.0 - nearestDist / awarenessRadius).clamp(0.0, 1.0);
    } else {
      _proximateAnchor = null;
      _anchorProximity = 0.0;
    }
  }

  _TriggerAnchor? _snappedAnchorForMode(RhythmMode mode, double hour) {
    final anchors = _triggerAnchorsForMode(mode)
        .where((anchor) => _isHourAllowedForMode(mode, anchor.hour))
        .toList();
    if (anchors.isEmpty) return null;

    _TriggerAnchor? bestAnchor;
    var bestDistance = double.infinity;

    for (final anchor in anchors) {
      final distance = (hour - anchor.hour).abs();
      if (distance < bestDistance) {
        bestDistance = distance;
        bestAnchor = anchor;
      }
    }

    if (bestDistance <= _eventSnapThresholdHours) {
      return bestAnchor;
    }
    return null;
  }

  bool _isHourAllowedForMode(RhythmMode mode, double hour) {
    final normalized = SolarUtils.normalizeHour(hour);
    final dayHour = _handleHourForMode(RhythmMode.day) ?? 6.0;
    final sleepHour = _handleHourForMode(RhythmMode.sleep) ?? 22.0;

    if (mode == RhythmMode.day) {
      return normalized <= sleepHour - _minimumHandleGapHours;
    }
    return normalized >= dayHour + _minimumHandleGapHours;
  }

  List<_TriggerAnchor> _triggerAnchorsForMode(RhythmMode mode) {
    final tw = _twilightTimes;
    final st = _sunTimes;
    if (tw == null || st == null) return const [];

    if (mode == RhythmMode.day) {
      return [
        if (tw.dawn.astronomical != null)
          _TriggerAnchor(
            event: 'astronomical_twilight',
            label: 'Astro Dawn',
            hour: tw.dawn.astronomical!,
          ),
        if (tw.dawn.nautical != null)
          _TriggerAnchor(
            event: 'nautical_twilight',
            label: 'Nautical Dawn',
            hour: tw.dawn.nautical!,
          ),
        if (tw.dawn.civil != null)
          _TriggerAnchor(
            event: 'civil_twilight',
            label: 'Civil Dawn',
            hour: tw.dawn.civil!,
          ),
        _TriggerAnchor(
          event: 'sunrise',
          label: 'Sunrise',
          hour: st.sunrise,
        ),
      ];
    }

    return [
      _TriggerAnchor(
        event: 'sunset',
        label: 'Sunset',
        hour: st.sunset,
      ),
      if (tw.dusk.civil != null)
        _TriggerAnchor(
          event: 'civil_twilight',
          label: 'Civil Dusk',
          hour: tw.dusk.civil!,
        ),
      if (tw.dusk.nautical != null)
        _TriggerAnchor(
          event: 'nautical_twilight',
          label: 'Nautical Dusk',
          hour: tw.dusk.nautical!,
        ),
      if (tw.dusk.astronomical != null)
        _TriggerAnchor(
          event: 'astronomical_twilight',
          label: 'Astro Dusk',
          hour: tw.dusk.astronomical!,
        ),
    ];
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

class _TransitionHandleSpec {
  final RhythmMode mode;
  final double hour;
  final Color color;

  const _TransitionHandleSpec({
    required this.mode,
    required this.hour,
    required this.color,
  });
}

class _TriggerAnchor {
  final String event;
  final String label;
  final double hour;

  const _TriggerAnchor({
    required this.event,
    required this.label,
    required this.hour,
  });
}

class _ModeCurveVisual {
  final SolarCurveSamples? samples;
  final Color fallbackColor;
  final int fallbackBrightness;

  const _ModeCurveVisual({
    this.samples,
    required this.fallbackColor,
    this.fallbackBrightness = 50,
  });

  (Color, double) styleAt(double hour) {
    final curveSamples = samples;
    if (curveSamples == null || curveSamples.isEmpty) {
      final opacity =
          0.18 + (fallbackBrightness.clamp(0, 100).toDouble() / 100) * 0.55;
      return (fallbackColor, opacity);
    }

    final brightness = SolarUtils.interpolateValue(
      curveSamples.hours,
      curveSamples.brightness,
      hour,
    );
    final color = SolarUtils.curveColorAt(
      hour,
      hours: curveSamples.hours,
      kelvin: curveSamples.kelvin,
      fallback: fallbackColor,
    );
    final opacity = 0.18 + (brightness / 100) * 0.55;
    return (color, opacity);
  }
}

class _ClockModeOrb extends StatelessWidget {
  final RhythmMode mode;
  final Color color;
  final String timeLabel;
  final bool selected;

  const _ClockModeOrb({
    required this.mode,
    required this.color,
    required this.timeLabel,
    required this.selected,
  });

  @override
  Widget build(BuildContext context) {
    final width = selected ? 70.0 : 62.0;
    final orbSize = selected ? 44.0 : 40.0;

    return SizedBox(
      width: width,
      child: Container(
        width: orbSize,
        height: orbSize,
        decoration: BoxDecoration(
          shape: BoxShape.circle,
          color: Color.lerp(
            const Color(0xFF1A1A2E),
            color,
            selected ? 0.35 : 0.25,
          ),
          border: Border.all(
            color: color.withValues(alpha: selected ? 0.85 : 0.6),
            width: selected ? 2.0 : 1.6,
          ),
          boxShadow: [
            BoxShadow(
              color: color.withValues(alpha: selected ? 0.45 : 0.25),
              blurRadius: selected ? 18 : 12,
            ),
          ],
        ),
        child: Column(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Icon(
              _modeIcon(mode),
              size: selected ? 18 : 16,
              color: color,
            ),
            const SizedBox(height: 1),
            Text(
              timeLabel,
              style: TextStyle(
                color: Colors.white.withValues(alpha: selected ? 0.82 : 0.6),
                fontSize: 8.5,
                fontWeight: FontWeight.w600,
                letterSpacing: 0.1,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _ClockHandlesPainter extends CustomPainter {
  final SolarClockGeometry geometry;
  final List<_TransitionHandleSpec> handles;
  final RhythmMode selectedMode;
  final double flowProgress;

  const _ClockHandlesPainter({
    required this.geometry,
    required this.handles,
    required this.selectedMode,
    required this.flowProgress,
  });

  @override
  void paint(Canvas canvas, Size size) {
    for (final handle in handles) {
      final isSelected = handle.mode == selectedMode;
      if (!isSelected) continue;

      final anchor = geometry.positionForHour(handle.hour);

      // Glow behind the selected orb
      canvas.drawCircle(
        anchor,
        24,
        Paint()
          ..color = handle.color.withValues(alpha: 0.22)
          ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 14),
      );
    }
  }

  @override
  bool shouldRepaint(covariant _ClockHandlesPainter oldDelegate) {
    return geometry != oldDelegate.geometry ||
        handles != oldDelegate.handles ||
        selectedMode != oldDelegate.selectedMode ||
        flowProgress != oldDelegate.flowProgress;
  }
}

class _ClockSolarAnchorPainter extends CustomPainter {
  final SolarClockGeometry geometry;
  final List<_TriggerAnchor> anchors;
  final Color accentColor;
  final String? activeEvent;
  final _TriggerAnchor? proximateAnchor;
  final double anchorProximity;
  final double handleHour;
  final double handleWidth;
  final double? dragHour;
  final double pulse;
  final bool isDragging;

  const _ClockSolarAnchorPainter({
    required this.geometry,
    required this.anchors,
    required this.accentColor,
    required this.activeEvent,
    required this.proximateAnchor,
    required this.anchorProximity,
    required this.handleHour,
    required this.handleWidth,
    required this.dragHour,
    required this.pulse,
    required this.isDragging,
  });

  @override
  void paint(Canvas canvas, Size size) {
    if (anchors.isEmpty) return;

    final layouts = <_ClockSolarAnchorLayout>[];

    for (final anchor in anchors) {
      final anchorPoint = geometry.positionForHour(anchor.hour);
      final radial = anchorPoint - geometry.center;
      final radialDistance = radial.distance;
      if (radialDistance == 0) continue;

      final normal = radial / radialDistance;
      final isActive = !isDragging && activeEvent == anchor.event;
      final isProximate = isDragging && proximateAnchor?.event == anchor.event;
      final dragFocus = isDragging && dragHour != null
          ? (1.0 - (dragHour! - anchor.hour).abs() / 1.35).clamp(0.0, 1.0)
          : 0.0;
      final emphasis = isActive
          ? 1.0
          : math.max(
              isProximate ? anchorProximity : 0.0,
              dragFocus * 0.92,
            );
      final zoom = 1.0 + emphasis * (0.75 + pulse * 0.35);
      final tickExtent = 9.0 + zoom * 4.2;
      final strokeWidth = 2.6 + emphasis * 2.0;
      final tickColor = Color.lerp(
        accentColor,
        Colors.white,
        0.14 + emphasis * 0.26,
      )!;
      final textPainter = TextPainter(
        text: TextSpan(
          text: anchor.label,
          style: TextStyle(
            color: tickColor.withValues(alpha: 0.68 + emphasis * 0.28),
            fontSize: 8.8 + emphasis * 1.2,
            fontWeight: FontWeight.w700,
            letterSpacing: 0.1,
            shadows: [
              Shadow(
                color: Colors.black.withValues(alpha: 0.45),
                blurRadius: 6,
              ),
            ],
          ),
        ),
        textDirection: TextDirection.ltr,
      )..layout();
      final inner = geometry.center + normal * (geometry.radius - tickExtent);
      final outer = geometry.center + normal * (geometry.radius + tickExtent);
      final innerLeader =
          geometry.center + normal * (geometry.radius - tickExtent - 10);

      canvas.drawLine(
        inner,
        outer,
        Paint()
          ..color = tickColor.withValues(alpha: 0.16 + emphasis * 0.18)
          ..strokeWidth = strokeWidth * (3.8 + emphasis * 0.5)
          ..strokeCap = StrokeCap.round
          ..maskFilter = MaskFilter.blur(
            BlurStyle.normal,
            5 + emphasis * 4,
          ),
      );
      canvas.drawLine(
        inner,
        outer,
        Paint()
          ..color = tickColor.withValues(alpha: 0.82 + emphasis * 0.18)
          ..strokeWidth = strokeWidth
          ..strokeCap = StrokeCap.round,
      );
      canvas.drawCircle(
        outer,
        2.2 + emphasis * (3.6 + pulse * 1.8),
        Paint()
          ..color = tickColor.withValues(alpha: 0.16 + emphasis * 0.16)
          ..maskFilter = MaskFilter.blur(
            BlurStyle.normal,
            4 + emphasis * 4,
          ),
      );
      canvas.drawCircle(
        outer,
        1.2 + emphasis * 1.4,
        Paint()..color = tickColor.withValues(alpha: 0.75 + emphasis * 0.20),
      );

      layouts.add(
        _ClockSolarAnchorLayout(
          leaderStart: innerLeader,
          normal: normal,
          tickColor: tickColor,
          emphasis: emphasis,
          textPainter: textPainter,
          preferredTop: anchorPoint.dy - textPainter.height / 2,
        ),
      );
    }

    if (layouts.isEmpty) return;

    final leftLayouts =
        layouts.where((layout) => layout.normal.dx < 0).toList();
    final rightLayouts =
        layouts.where((layout) => layout.normal.dx >= 0).toList();

    _paintClockSolarAnchorSide(
      canvas,
      size,
      geometry: geometry,
      layouts: leftLayouts,
      handleHour: handleHour,
      handleWidth: handleWidth,
      isRightSide: false,
    );
    _paintClockSolarAnchorSide(
      canvas,
      size,
      geometry: geometry,
      layouts: rightLayouts,
      handleHour: handleHour,
      handleWidth: handleWidth,
      isRightSide: true,
    );
  }

  @override
  bool shouldRepaint(covariant _ClockSolarAnchorPainter oldDelegate) {
    return geometry != oldDelegate.geometry ||
        anchors != oldDelegate.anchors ||
        accentColor != oldDelegate.accentColor ||
        activeEvent != oldDelegate.activeEvent ||
        proximateAnchor != oldDelegate.proximateAnchor ||
        anchorProximity != oldDelegate.anchorProximity ||
        handleHour != oldDelegate.handleHour ||
        handleWidth != oldDelegate.handleWidth ||
        dragHour != oldDelegate.dragHour ||
        pulse != oldDelegate.pulse ||
        isDragging != oldDelegate.isDragging;
  }
}

class _ClockSolarAnchorLayout {
  final Offset leaderStart;
  final Offset normal;
  final Color tickColor;
  final double emphasis;
  final TextPainter textPainter;
  final double preferredTop;
  double top;

  _ClockSolarAnchorLayout({
    required this.leaderStart,
    required this.normal,
    required this.tickColor,
    required this.emphasis,
    required this.textPainter,
    required this.preferredTop,
  }) : top = preferredTop;
}

void _paintClockSolarAnchorSide(
  Canvas canvas,
  Size size, {
  required SolarClockGeometry geometry,
  required List<_ClockSolarAnchorLayout> layouts,
  required double handleHour,
  required double handleWidth,
  required bool isRightSide,
}) {
  if (layouts.isEmpty) return;

  final maxTextWidth = layouts.fold<double>(
    0.0,
    (maxWidth, item) => math.max(maxWidth, item.textPainter.width),
  );
  final handleCenter = geometry.positionForHour(handleHour);
  final handleOnRight = handleCenter.dx >= geometry.center.dx;
  const handleGap = 16.0;
  final preferredColumnLeft = isRightSide
      ? geometry.center.dx + geometry.radius * 0.06
      : geometry.center.dx - geometry.radius * 0.06 - maxTextWidth;
  var columnLeft = preferredColumnLeft;

  if (isRightSide && handleOnRight) {
    columnLeft = math.min(
      columnLeft,
      handleCenter.dx - handleWidth / 2 - maxTextWidth - handleGap,
    );
  } else if (!isRightSide && !handleOnRight) {
    columnLeft = math.max(
      columnLeft,
      handleCenter.dx + handleWidth / 2 + handleGap,
    );
  }

  final minColumnLeft = isRightSide ? geometry.center.dx - 10.0 : 12.0;
  final maxColumnLeft = isRightSide
      ? size.width - maxTextWidth - 12.0
      : geometry.center.dx - maxTextWidth * 0.35;
  columnLeft = columnLeft.clamp(minColumnLeft, maxColumnLeft).toDouble();

  _resolveClockSolarAnchorLabelTops(
    layouts,
    minTop: math.max(14.0, geometry.center.dy - geometry.radius * 0.78),
    maxBottom: math.min(
      size.height - 14.0,
      geometry.center.dy + geometry.radius * 0.78,
    ),
    gap: 10.0,
  );

  for (final layout in layouts) {
    final labelLeft = isRightSide
        ? columnLeft
        : columnLeft + (maxTextWidth - layout.textPainter.width);
    final labelCenterY = layout.top + layout.textPainter.height / 2;
    final connectorEnd = Offset(
      isRightSide
          ? labelLeft + layout.textPainter.width + 6.0
          : labelLeft - 6.0,
      labelCenterY,
    );

    canvas.drawLine(
      layout.leaderStart,
      connectorEnd,
      Paint()
        ..color = layout.tickColor.withValues(
          alpha: 0.16 + layout.emphasis * 0.14,
        )
        ..strokeWidth = 1.2 + layout.emphasis * 0.7
        ..strokeCap = StrokeCap.round,
    );

    layout.textPainter.paint(canvas, Offset(labelLeft, layout.top));
  }
}

void _resolveClockSolarAnchorLabelTops(
  List<_ClockSolarAnchorLayout> layouts, {
  required double minTop,
  required double maxBottom,
  required double gap,
}) {
  layouts.sort((a, b) => a.preferredTop.compareTo(b.preferredTop));

  for (var i = 0; i < layouts.length; i++) {
    final layout = layouts[i];
    var nextTop = layout.preferredTop.clamp(
      minTop,
      maxBottom - layout.textPainter.height,
    );
    if (i > 0) {
      final previous = layouts[i - 1];
      final minAllowedTop = previous.top + previous.textPainter.height + gap;
      if (nextTop < minAllowedTop) nextTop = minAllowedTop;
    }
    layout.top = nextTop;
  }

  final overflow =
      layouts.last.top + layouts.last.textPainter.height - maxBottom;
  if (overflow > 0) {
    layouts.last.top -= overflow;
    for (var i = layouts.length - 2; i >= 0; i--) {
      final current = layouts[i];
      final next = layouts[i + 1];
      final maxAllowedTop = next.top - current.textPainter.height - gap;
      if (current.top > maxAllowedTop) current.top = maxAllowedTop;
    }
  }

  final underflow = minTop - layouts.first.top;
  if (underflow > 0) {
    for (final layout in layouts) {
      layout.top += underflow;
    }
  }
}

class _RhythmClockRingPainter extends CustomPainter {
  final SolarClockGeometry geometry;
  final double dayStartHour;
  final double sleepStartHour;
  final _ModeCurveVisual dayVisual;
  final _ModeCurveVisual sleepVisual;
  final RhythmMode selectedMode;
  final bool use24;

  const _RhythmClockRingPainter({
    required this.geometry,
    required this.dayStartHour,
    required this.sleepStartHour,
    required this.dayVisual,
    required this.sleepVisual,
    required this.selectedMode,
    required this.use24,
  });

  @override
  void paint(Canvas canvas, Size size) {
    _drawRing(canvas);
    _drawClockTicks(canvas);
    _drawHourLabels(canvas);
  }

  void _drawRing(Canvas canvas) {
    const segments = 144;
    final rect = geometry.arcRect;
    final paint = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 16
      ..strokeCap = StrokeCap.round;
    final glowPaint = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 24
      ..strokeCap = StrokeCap.round
      ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 10);
    final accentPaint = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 18
      ..strokeCap = StrokeCap.round;
    const hourStep = 24.0 / segments;
    final sweep = (2 * math.pi) / segments;
    final selectedIsDay = selectedMode == RhythmMode.day;

    for (int i = 0; i < segments; i++) {
      final startHour = i * hourStep;
      final midHour = startHour + hourStep / 2;
      final isDayHour = _isWithinDaySegment(
        midHour,
        dayStartHour,
        sleepStartHour,
      );
      final (color, opacity) =
          (isDayHour ? dayVisual : sleepVisual).styleAt(midHour);
      final baseOpacity = isDayHour ? opacity : math.max(opacity, 0.38);
      final isSelectedSegment = isDayHour == selectedIsDay;
      // Let the curve color render at its natural opacity on both halves so
      // the ring actually reads as the rhythm curve. Selected gets a small
      // brightness bump on top; differentiation also comes from the glow +
      // accent stroke layers below.
      final effectiveOpacity = isSelectedSegment
          ? math.min(0.98, baseOpacity * 1.05 + 0.08)
          : baseOpacity;
      final accentColor = Color.lerp(color, Colors.white, 0.10)!;
      final startAngle = geometry.angleForHour(startHour);

      if (isSelectedSegment) {
        glowPaint.color =
            color.withValues(alpha: math.min(0.24, 0.06 + baseOpacity * 0.18));
        canvas.drawArc(
          rect,
          startAngle,
          sweep + 0.02,
          false,
          glowPaint,
        );
      }

      paint.color = color.withValues(alpha: effectiveOpacity);
      canvas.drawArc(
        rect,
        startAngle,
        sweep + 0.02,
        false,
        paint,
      );

      if (isSelectedSegment) {
        accentPaint.color = accentColor.withValues(
          alpha: math.min(0.34, 0.12 + baseOpacity * 0.18),
        );
        canvas.drawArc(
          rect,
          startAngle,
          sweep + 0.02,
          false,
          accentPaint,
        );
      }
    }
  }

  /// Draws watch-style index marks around the ring perimeter.
  ///
  /// Hierarchy mirrors a traditional clock face:
  /// - Cardinal (0, 6, 12, 18) — tallest, boldest
  /// - 3-hour (3, 9, 15, 21) — medium
  /// - Hourly — short
  /// - Half-hour — subtle subdivisions
  void _drawClockTicks(Canvas canvas) {
    final ringOuter = geometry.radius + 8;
    final tickPaint = Paint()..strokeCap = StrokeCap.round;

    for (int i = 0; i < 48; i++) {
      final hour = i * 0.5;
      final isWholeHour = i.isEven;
      final hourInt = i ~/ 2;

      // Skip ticks at labeled positions — the number serves as the marker
      if (isWholeHour && hourInt % 3 == 0) continue;

      final angle = geometry.angleForHour(hour);
      final cosA = math.cos(angle);
      final sinA = math.sin(angle);

      double tickLen, strokeW, alpha;

      if (isWholeHour) {
        tickLen = 3.5;
        strokeW = 1.0;
        alpha = 0.18;
      } else {
        tickLen = 2.0;
        strokeW = 0.7;
        alpha = 0.10;
      }

      final innerR = ringOuter + 1.5;
      final outerR = ringOuter + 1.5 + tickLen;

      tickPaint
        ..color = Colors.white.withValues(alpha: alpha)
        ..strokeWidth = strokeW;

      canvas.drawLine(
        Offset(
          geometry.center.dx + innerR * cosA,
          geometry.center.dy + innerR * sinA,
        ),
        Offset(
          geometry.center.dx + outerR * cosA,
          geometry.center.dy + outerR * sinA,
        ),
        tickPaint,
      );
    }
  }

  void _drawHourLabels(Canvas canvas) {
    const labeledHours = [0, 3, 6, 9, 12, 15, 18, 21];

    for (final hour in labeledHours) {
      final angle = geometry.angleForHour(hour.toDouble());
      final cosA = math.cos(angle);
      final sinA = math.sin(angle);
      final isCardinal = hour % 6 == 0;

      final label = use24
          ? hour.toString().padLeft(2, '0')
          : hour == 0
              ? '12 AM'
              : hour == 12
                  ? '12 PM'
                  : hour < 12
                      ? '$hour AM'
                      : '${hour - 12} PM';

      final textPainter = TextPainter(
        text: TextSpan(
          text: label,
          style: TextStyle(
            color: Colors.white.withValues(alpha: isCardinal ? 0.72 : 0.42),
            fontSize: isCardinal ? 12 : 10,
            fontWeight: isCardinal ? FontWeight.w600 : FontWeight.w500,
            letterSpacing: 0.3,
          ),
        ),
        textDirection: TextDirection.ltr,
      )..layout();

      final distance = geometry.radius + 22;
      textPainter.paint(
        canvas,
        Offset(
          geometry.center.dx + distance * cosA - textPainter.width / 2,
          geometry.center.dy + distance * sinA - textPainter.height / 2,
        ),
      );
    }
  }

  bool _isWithinDaySegment(double hour, double dayStart, double sleepStart) {
    final normalizedHour = SolarUtils.normalizeHour(hour);
    final normalizedDayStart = SolarUtils.normalizeHour(dayStart);
    final normalizedSleepStart = SolarUtils.normalizeHour(sleepStart);

    if (normalizedDayStart <= normalizedSleepStart) {
      return normalizedHour >= normalizedDayStart &&
          normalizedHour < normalizedSleepStart;
    }

    return normalizedHour >= normalizedDayStart ||
        normalizedHour < normalizedSleepStart;
  }

  @override
  bool shouldRepaint(covariant _RhythmClockRingPainter oldDelegate) {
    return geometry != oldDelegate.geometry ||
        dayStartHour != oldDelegate.dayStartHour ||
        sleepStartHour != oldDelegate.sleepStartHour ||
        dayVisual != oldDelegate.dayVisual ||
        sleepVisual != oldDelegate.sleepVisual ||
        selectedMode != oldDelegate.selectedMode ||
        use24 != oldDelegate.use24;
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

String _shortAnchorLabel(_TriggerAnchor anchor) {
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

/// A `PanGestureRecognizer` that refuses to lose the gesture arena. Used
/// for the orbital clock so the orbs stay draggable even when an ancestor
/// scroll view tries to claim vertical drags (e.g. once the page's content
/// overflows after the Save/Revert cluster expands).
class _ClockPanGestureRecognizer extends PanGestureRecognizer {
  @override
  void rejectGesture(int pointer) {
    acceptGesture(pointer);
  }
}
