import 'dart:async';
import 'dart:convert';

import 'package:crypto/crypto.dart';
import 'package:flutter/widgets.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:uuid/uuid.dart';

import 'analytics_service.dart';
import 'apple_health_service.dart';

class AppleHealthScope {
  final String accountId;
  final String homeId;
  final String serverId;
  final String nodeId;

  const AppleHealthScope(
      {required this.accountId,
      required this.homeId,
      required this.serverId,
      required this.nodeId});

  Map<String, String> toJson() => {
        'account_id': accountId,
        'home_id': homeId,
        'server_id': serverId,
        'node_id': nodeId
      };

  static AppleHealthScope? fromJson(Object? value) {
    if (value is! Map) return null;
    for (final key in ['account_id', 'home_id', 'server_id', 'node_id']) {
      if (value[key] is! String ||
          (value[key] as String).isEmpty ||
          (value[key] as String).length > 256) {
        return null;
      }
    }
    return AppleHealthScope(
        accountId: value['account_id'] as String,
        homeId: value['home_id'] as String,
        serverId: value['server_id'] as String,
        nodeId: value['node_id'] as String);
  }

  bool sameEnvironment(AppleHealthScope? other) =>
      other != null &&
      accountId == other.accountId &&
      homeId == other.homeId &&
      serverId == other.serverId;

  @override
  bool operator ==(Object other) =>
      other is AppleHealthScope &&
      sameEnvironment(other) &&
      nodeId == other.nodeId;
  @override
  int get hashCode => Object.hash(accountId, homeId, serverId, nodeId);
}

class AppleHealthHistorySnapshot {
  final List<RhythmActivityEvent> events;
  final bool partial;
  const AppleHealthHistorySnapshot(
      {required this.events, this.partial = false});
}

enum AppleHealthSyncStatus {
  disabled,
  ready,
  syncing,
  synced,
  empty,
  denied,
  unavailable,
  offline,
  partial,
  failed
}

/// Device-local consent and retry owner. The retained activity remains the
/// authority; no local timestamp watermark can suppress a retry or late upload.
class AppleHealthSyncController extends ChangeNotifier
    with WidgetsBindingObserver {
  final AppleHealthWriter writer;
  final AppleHealthScope? Function() currentEnvironment;
  final Object? Function() readPreference;
  final Future<void> Function(Map<String, Object>? value) writePreference;
  final Future<AppleHealthHistorySnapshot> Function(
      AppleHealthScope scope, int cutoff) loadHistory;
  final DateTime Function() now;
  final bool Function()? isConnected;

  AppleHealthScope? _scope;
  bool _enabled = false;
  bool _disposed = false;
  bool _foreground = true;
  bool _retryAfterFlight = false;
  bool _wasConnected = false;
  int _generation = 0;
  Future<void>? _running;
  Timer? _retryTimer;
  final List<Listenable> _triggers = [];
  AppleHealthSyncStatus status = AppleHealthSyncStatus.disabled;

  AppleHealthSyncController(
      {required this.writer,
      required this.currentEnvironment,
      required this.readPreference,
      required this.writePreference,
      required this.loadHistory,
      this.isConnected,
      DateTime Function()? now})
      : now = now ?? DateTime.now {
    final stored = readPreference();
    if (stored is Map && stored['schema_version'] == 1) {
      _scope = AppleHealthScope.fromJson(stored['scope']);
      _enabled = _scope != null && stored['enabled'] == true;
    }
    status =
        _enabled ? AppleHealthSyncStatus.ready : AppleHealthSyncStatus.disabled;
  }

  AppleHealthScope? get scope => _scope;
  bool get enabled => _enabled;
  bool get busy => _running != null;
  bool get matchesCurrentHome =>
      _scope?.sameEnvironment(currentEnvironment()) == true;

  void start(List<Listenable> triggers) {
    WidgetsBinding.instance.addObserver(this);
    _foreground = WidgetsBinding.instance.lifecycleState == null ||
        WidgetsBinding.instance.lifecycleState == AppLifecycleState.resumed;
    for (final trigger in triggers) {
      _triggers.add(trigger);
      trigger.addListener(environmentChanged);
    }
    // Provider creation must not notify while the widget tree is building.
    scheduleMicrotask(environmentChanged);
    _retryTimer =
        Timer.periodic(const Duration(minutes: 5), (_) => unawaited(sync()));
  }

  void environmentChanged() {
    if (_disposed) return;
    final connected = isConnected?.call() ?? false;
    final reconnected = connected && !_wasConnected;
    _wasConnected = connected;
    if (!_refreshConsent()) return;
    if (!matchesCurrentHome) {
      _generation++;
      _setStatus(AppleHealthSyncStatus.offline);
      return;
    }
    if (_foreground &&
        (status == AppleHealthSyncStatus.ready ||
            status == AppleHealthSyncStatus.offline ||
            reconnected)) {
      if (busy) {
        _retryAfterFlight = true;
      } else {
        unawaited(sync());
      }
    }
  }

  bool _refreshConsent() {
    if (_enabled && !_storedConsentMatches()) {
      _generation++;
      _enabled = false;
      _scope = null;
      _setStatus(AppleHealthSyncStatus.disabled);
      return false;
    }
    return _enabled;
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    // The system authorization sheet makes the app inactive temporarily.
    if (state == AppLifecycleState.inactive) return;
    _foreground = state == AppLifecycleState.resumed;
    if (!_foreground) {
      _generation++;
      return;
    }
    if (_enabled && matchesCurrentHome) {
      if (busy) {
        _retryAfterFlight = true;
      } else {
        unawaited(sync());
      }
    }
  }

  Future<void> enable(AppleHealthScope selected) async {
    if (busy || !selected.sameEnvironment(currentEnvironment())) return;
    final generation = ++_generation;
    final journey = 'apple-health-${const Uuid().v4()}';
    unawaited(AnalyticsService().logAppleHealthAction('enable', journey));
    _setStatus(AppleHealthSyncStatus.syncing);
    final operation = _enable(selected, generation, journey);
    _running = operation;
    try {
      await operation;
    } finally {
      _finishFlight();
    }
    if (_enabled && selected == _scope && _valid(generation, selected)) {
      await sync();
    }
  }

  Future<void> _enable(
      AppleHealthScope selected, int generation, String journey) async {
    var stage = 'authorization';
    try {
      final authorization = await writer.requestAuthorization();
      if (!_canComplete(generation, selected, journey)) return;
      if (authorization != AppleHealthAuthorization.authorized) {
        _authorizationStatus(authorization);
        _outcome(journey, 'failed', 'authorization');
        return;
      }
      stage = 'preference';
      await writePreference(
          {'schema_version': 1, 'enabled': true, 'scope': selected.toJson()});
      if (!_valid(generation, selected)) {
        await writePreference(null);
        _outcome(journey, 'cancelled', 'none');
        return;
      }
      _scope = selected;
      _enabled = true;
      _setStatus(AppleHealthSyncStatus.ready);
      _outcome(journey, 'succeeded', 'none');
    } catch (_) {
      if (_valid(generation, selected)) {
        _setStatus(AppleHealthSyncStatus.failed);
        _outcome(journey, 'failed', stage);
      } else {
        _outcome(journey, 'cancelled', 'none');
      }
    }
  }

  Future<void> disable() async {
    ++_generation;
    _enabled =
        false; // Fence pending native writes before awaiting persistence.
    _setStatus(AppleHealthSyncStatus.disabled);
    final journey = 'apple-health-${const Uuid().v4()}';
    unawaited(AnalyticsService().logAppleHealthAction('disable', journey));
    try {
      await writePreference(null);
      _scope = null;
      _outcome(journey, 'succeeded', 'none');
    } catch (_) {
      _setStatus(AppleHealthSyncStatus.failed);
      _outcome(journey, 'failed', 'preference');
    }
  }

  Future<void> sync() async {
    if (_disposed || !_enabled || !_foreground || busy) return;
    if (!_refreshConsent()) return;
    final selected = _scope!;
    if (!matchesCurrentHome) {
      _setStatus(AppleHealthSyncStatus.offline);
      return;
    }
    final generation = _generation;
    final journey = 'apple-health-${const Uuid().v4()}';
    unawaited(AnalyticsService().logAppleHealthAction('sync', journey));
    _setStatus(AppleHealthSyncStatus.syncing);
    final operation = _sync(selected, generation, journey);
    _running = operation;
    try {
      await operation;
    } finally {
      _finishFlight();
    }
  }

  Future<void> _sync(
      AppleHealthScope selected, int generation, String journey) async {
    var stage = 'authorization';
    try {
      final authorization = await writer.authorization();
      if (!_canComplete(generation, selected, journey)) return;
      if (authorization != AppleHealthAuthorization.authorized) {
        _authorizationStatus(authorization);
        _outcome(journey, 'failed', stage);
        return;
      }
      stage = 'history';
      final cutoff = now().millisecondsSinceEpoch;
      final snapshot = await loadHistory(selected, cutoff);
      if (!_canComplete(generation, selected, journey)) return;
      final intervals = RhythmSleepInterval.fromActivity(snapshot.events,
          nodeId: selected.nodeId, nowEpochMs: cutoff);
      stage = 'health_write';
      for (var offset = 0; offset < intervals.length; offset += 100) {
        if (!_canComplete(generation, selected, journey) || !_enabled) return;
        final batch = intervals.skip(offset).take(100).map((interval) {
          final identifier = sha256
              .convert(utf8.encode(jsonEncode([
                selected.accountId,
                selected.homeId,
                selected.serverId,
                selected.nodeId,
                interval.sleepEventId,
              ])))
              .toString();
          return <String, Object>{
            'sync_id': 'rhythm-in-bed-$identifier',
            'start_epoch_ms': interval.startEpochMs,
            'end_epoch_ms': interval.endEpochMs
          };
        }).toList();
        await writer.saveIntervals(batch).timeout(const Duration(seconds: 30));
      }
      if (!_canComplete(generation, selected, journey)) return;
      _setStatus(snapshot.partial
          ? AppleHealthSyncStatus.partial
          : intervals.isEmpty
              ? AppleHealthSyncStatus.empty
              : AppleHealthSyncStatus.synced);
      _outcome(journey, snapshot.partial ? 'partial' : 'succeeded',
          snapshot.partial ? 'history' : 'none');
    } catch (_) {
      if (!_canComplete(generation, selected, journey)) return;
      _setStatus(stage == 'history'
          ? AppleHealthSyncStatus.offline
          : AppleHealthSyncStatus.failed);
      _outcome(journey, 'failed', stage);
    }
  }

  bool _valid(int generation, AppleHealthScope selected) =>
      !_disposed &&
      _foreground &&
      generation == _generation &&
      selected.sameEnvironment(currentEnvironment()) &&
      (!_enabled || _storedConsentMatches());

  bool _storedConsentMatches() {
    final stored = readPreference();
    return stored is Map &&
        stored['schema_version'] == 1 &&
        stored['enabled'] == true &&
        AppleHealthScope.fromJson(stored['scope']) == _scope;
  }

  bool _canComplete(int generation, AppleHealthScope selected, String journey) {
    if (_valid(generation, selected)) return true;
    _outcome(journey, 'cancelled', 'none');
    return false;
  }

  void _finishFlight() {
    _running = null;
    if (_disposed) return;
    if (status == AppleHealthSyncStatus.syncing) {
      status = _enabled
          ? AppleHealthSyncStatus.ready
          : AppleHealthSyncStatus.disabled;
    }
    notifyListeners();
    if (_retryAfterFlight) {
      _retryAfterFlight = false;
      scheduleMicrotask(() => unawaited(sync()));
    }
  }

  void _authorizationStatus(AppleHealthAuthorization value) =>
      _setStatus(value == AppleHealthAuthorization.unavailable
          ? AppleHealthSyncStatus.unavailable
          : AppleHealthSyncStatus.denied);

  void _outcome(String journey, String outcome, String stage) {
    // This bounded diagnostic reaches existing app-log debug bundles. Never log
    // sleep timestamps/durations, source identity, or raw native exceptions.
    debugPrint('AppleHealth sync outcome=$outcome stage=$stage');
    unawaited(
        AnalyticsService().logAppleHealthOutcome(outcome, stage, journey));
  }

  void _setStatus(AppleHealthSyncStatus value) {
    status = value;
    debugPrint('AppleHealth sync status=${value.name}');
    if (!_disposed) notifyListeners();
  }

  @override
  void dispose() {
    _disposed = true;
    _generation++;
    _retryTimer?.cancel();
    for (final trigger in _triggers) {
      trigger.removeListener(environmentChanged);
    }
    WidgetsBinding.instance.removeObserver(this);
    super.dispose();
  }
}
