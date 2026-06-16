import 'dart:async';

import 'package:logging/logging.dart';

/// Root logger for the Rhythm SDK.
///
/// Consumers can attach handlers to this logger or its children
/// (`rhythm_sdk.connection`, `rhythm_sdk.api`, `rhythm_sdk.ota`)
/// to observe SDK internals.
final rhythmLogger = Logger('rhythm_sdk');

/// Convenience class for quick debug logging setup.
abstract final class RhythmSdk {
  static StreamSubscription<LogRecord>? _subscription;

  /// Enable console logging for all Rhythm SDK activity.
  ///
  /// ```dart
  /// RhythmSdk.enableLogging(); // defaults to Level.FINE
  /// RhythmSdk.enableLogging(level: Level.WARNING); // errors only
  /// ```
  ///
  /// For full control, use `package:logging` directly:
  /// ```dart
  /// Logger('rhythm_sdk').level = Level.ALL;
  /// Logger('rhythm_sdk').onRecord.listen((record) { ... });
  /// ```
  static void enableLogging({Level level = Level.FINE}) {
    hierarchicalLoggingEnabled = true;
    rhythmLogger.level = level;

    // Cancel previous listener to avoid double-printing.
    _subscription?.cancel();
    _subscription = rhythmLogger.onRecord.listen((record) {
      final error = record.error != null ? ' | ${record.error}' : '';
      // ignore: avoid_print
      print('${record.time.toIso8601String()} [${record.loggerName}] '
          '${record.level.name}: ${record.message}$error');
    });
  }

  /// Disable SDK console logging.
  static void disableLogging() {
    _subscription?.cancel();
    _subscription = null;
    rhythmLogger.level = Level.OFF;
  }
}
