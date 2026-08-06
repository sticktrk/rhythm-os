import 'dart:async';

import 'analytics_service.dart';

/// Process-local timing for the startup journey to All Rooms.
///
/// Milestones are deliberately best-effort: recording them must never delay a
/// frame, provider initialization, or room interaction.
class AppStartupPerformance {
  AppStartupPerformance._();

  static final AppStartupPerformance instance = AppStartupPerformance._();

  final Stopwatch _stopwatch = Stopwatch();
  bool _visibleRecorded = false;
  bool _interactiveRecorded = false;
  bool _showedCachedRooms = false;

  void start() {
    _stopwatch
      ..reset()
      ..start();
    _visibleRecorded = false;
    _interactiveRecorded = false;
    _showedCachedRooms = false;
  }

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
