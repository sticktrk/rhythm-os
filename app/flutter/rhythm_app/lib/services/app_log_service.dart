import 'dart:async';

import 'package:flutter/foundation.dart';

import '../data/local_data_source.dart';

/// Small rolling log captured inside the app for support bundles.
class AppLogService {
  AppLogService._();

  static final AppLogService instance = AppLogService._();

  static const _storageKey = 'app_logs_v1';
  static const _maxEntries = 600;
  static const _maxLineLength = 1200;
  static const _maxStoredChars = 240000;
  static const _flushDelay = Duration(seconds: 2);

  final List<String> _entries = [];
  DebugPrintCallback? _previousDebugPrint;
  Timer? _flushTimer;
  LocalDataSource? _dataSource;
  bool _installed = false;
  bool _storageInitialized = false;
  FlutterExceptionHandler? _previousFlutterError;

  void install() {
    if (_installed) return;
    _installed = true;

    _previousDebugPrint = debugPrint;
    debugPrint = (String? message, {int? wrapWidth}) {
      recordDebugPrint(message);
      _previousDebugPrint?.call(message, wrapWidth: wrapWidth);
    };

    _previousFlutterError = FlutterError.onError;
    FlutterError.onError = (FlutterErrorDetails details) {
      record(
        'FlutterError: ${details.exceptionAsString()}',
        level: 'ERROR',
      );
      final stack = details.stack?.toString().trim();
      if (stack != null && stack.isNotEmpty) {
        record(stack, level: 'STACK');
      }
      _previousFlutterError?.call(details);
    };

    record('AppLogService installed');
  }

  Future<void> initializeStorage({LocalDataSource? dataSource}) async {
    _dataSource = dataSource ?? LocalDataSource();
    if (!_dataSource!.isInitialized) return;

    final stored = _dataSource!.getSettingsValue(_storageKey);
    if (stored is List) {
      final existing = stored.whereType<String>().toList(growable: false);
      _entries
        ..insertAll(0, existing)
        ..removeWhere((entry) => entry.trim().isEmpty);
      _trimEntries();
    }

    _storageInitialized = true;
    await flush();
  }

  void recordDebugPrint(String? message) {
    if (message == null || message.isEmpty) return;
    for (final line in message.split('\n')) {
      record(line);
    }
  }

  void record(String message, {String level = 'INFO'}) {
    final trimmed = message.trimRight();
    if (trimmed.isEmpty) return;

    final safeLine = _redact(trimmed).replaceAll('\r', r'\r');
    final limitedLine = safeLine.length > _maxLineLength
        ? '${safeLine.substring(0, _maxLineLength)}...'
        : safeLine;
    final timestamp = DateTime.now().toUtc().toIso8601String();
    _entries.add('[$timestamp] [$level] $limitedLine');
    _trimEntries();
    _scheduleFlush();
  }

  Future<String> snapshotText() async {
    await flush();
    final generatedAt = DateTime.now().toUtc().toIso8601String();
    return [
      'Rhythm app log',
      'generated_at=$generatedAt',
      'entry_count=${_entries.length}',
      '',
      ..._entries,
      '',
    ].join('\n');
  }

  Future<void> flush() async {
    _flushTimer?.cancel();
    _flushTimer = null;
    if (!_storageInitialized || _dataSource?.isInitialized != true) return;

    try {
      await _dataSource!.saveSettingsValue(
        _storageKey,
        List<String>.unmodifiable(_entries),
      );
    } catch (_) {
      // Logging must never block app behavior.
    }
  }

  @visibleForTesting
  void resetForTesting() {
    _entries.clear();
    _flushTimer?.cancel();
    _flushTimer = null;
    _dataSource = null;
    _storageInitialized = false;
  }

  void _scheduleFlush() {
    if (!_storageInitialized || _dataSource?.isInitialized != true) return;
    _flushTimer?.cancel();
    _flushTimer = Timer(_flushDelay, () {
      unawaited(flush());
    });
  }

  void _trimEntries() {
    while (_entries.length > _maxEntries) {
      _entries.removeAt(0);
    }

    var totalChars = _entries.fold<int>(0, (sum, entry) => sum + entry.length);
    while (_entries.isNotEmpty && totalChars > _maxStoredChars) {
      totalChars -= _entries.removeAt(0).length;
    }
  }

  String _redact(String value) {
    var output = value.replaceAllMapped(
      RegExp(r'(Bearer\s+)[^\s,;]+', caseSensitive: false),
      (match) => '${match.group(1)}[REDACTED]',
    );
    output = output.replaceAllMapped(
      RegExp(
        r'''((?:access|refresh|owner|connector|auth)[_-]?token["']?\s*[:=]\s*["']?)[^"',\s}]+''',
        caseSensitive: false,
      ),
      (match) => '${match.group(1)}[REDACTED]',
    );
    return output;
  }
}
