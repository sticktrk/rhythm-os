import 'dart:async';
import 'dart:math';

import 'analytics_service.dart';

/// Process-local timing for the startup journey to All Rooms.
///
/// Milestones are deliberately best-effort: recording them must never delay a
/// frame, provider initialization, or room interaction.
enum AppStartupPhase {
  settings,
  auth,
  brain,
  endpoint,
  request,
  decode,
  models,
  apply
}

class AppStartupPerformance {
  AppStartupPerformance._();

  static final AppStartupPerformance instance = AppStartupPerformance._();

  final Stopwatch _stopwatch = Stopwatch();
  bool _visibleRecorded = false;
  bool _interactiveRecorded = false;
  bool _showedCachedRooms = false;

  final Map<String, Object> _details = {};
  int _generation = 0;
  int get generation => _generation;

  void start({bool resumed = false}) {
    _generation++;
    final random = Random();
    _details
      ..clear()
      ..addAll({
        'journey_kind': resumed ? 'resume' : 'cold',
        'journey_id':
            '${random.nextInt(0x7fffffff)}-${random.nextInt(0x7fffffff)}',
      });
    _stopwatch
      ..reset()
      ..start();
    _visibleRecorded = false;
    _interactiveRecorded = false;
    _showedCachedRooms = false;
  }

  Future<T> measure<T>(AppStartupPhase phase, Future<T> Function() work) async {
    final generation = _generation;
    final timer = Stopwatch()..start();
    try {
      return await work();
    } finally {
      if (generation == _generation) {
        recordPhase(phase, timer.elapsedMilliseconds);
      }
    }
  }

  void recordPhase(AppStartupPhase phase, int elapsedMs) {
    if (_interactiveRecorded) return;
    _details['${phase.name}_ms'] = elapsedMs;
  }

  void recordServer(
      {required int nodes,
      required int devices,
      required String version,
      int? responseBytes,
      String? stateScope,
      String? transport}) {
    if (_interactiveRecorded) return;
    _details['node_count_bucket'] = countBucket(nodes);
    _details['device_count_bucket'] = countBucket(devices);
    _details['appliance_version'] = version;
    if (stateScope != null) _details['state_scope'] = stateScope;
    if (transport != null) _details['transport'] = transport;
    if (responseBytes != null) {
      _details['payload_size_bucket'] = responseBytes < 16 * 1024
          ? 'under_16k'
          : responseBytes < 64 * 1024
              ? '16k_64k'
              : responseBytes < 256 * 1024
                  ? '64k_256k'
                  : '256k_plus';
    }
  }

  static String countBucket(int count) => count == 0
      ? '0'
      : count <= 10
          ? '1_10'
          : count <= 50
              ? '11_50'
              : count <= 100
                  ? '51_100'
                  : count <= 200
                      ? '101_200'
                      : '201_plus';

  void markAllRoomsVisible({required bool fromCache, required int roomCount}) {
    if (_visibleRecorded) return;
    _ensureStarted();
    _visibleRecorded = true;
    _showedCachedRooms = fromCache;
    unawaited(
      AnalyticsService().logStartupAllRoomsVisible(
        elapsedMs: _stopwatch.elapsedMilliseconds,
        fromCache: fromCache,
        roomCountBucket: _roomCountBucket(roomCount),
      ),
    );
  }

  void markAllRoomsInteractive({required int roomCount}) {
    if (_interactiveRecorded) return;
    _ensureStarted();
    _interactiveRecorded = true;
    unawaited(AnalyticsService().logControlReadiness({
      ..._details,
      'elapsed_ms': _stopwatch.elapsedMilliseconds,
      'room_count_bucket': countBucket(roomCount),
      'showed_cached_rooms': _showedCachedRooms ? 1 : 0,
    }));
    unawaited(
      AnalyticsService().logStartupAllRoomsInteractive(
        elapsedMs: _stopwatch.elapsedMilliseconds,
        showedCachedRooms: _showedCachedRooms,
        roomCountBucket: _roomCountBucket(roomCount),
      ),
    );
  }

  void _ensureStarted() {
    if (_stopwatch.isRunning) return;
    _stopwatch.start();
  }

  String _roomCountBucket(int count) {
    if (count <= 0) return '0';
    if (count == 1) return '1';
    if (count <= 4) return '2_4';
    return '5_plus';
  }
}
