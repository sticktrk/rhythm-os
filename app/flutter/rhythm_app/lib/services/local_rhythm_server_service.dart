import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' show RhythmDiagnosticsApi;

const int localRhythmServerDefaultPort = 54448;
const String localRhythmServerHost = '127.0.0.1';

class LocalRhythmServerStatus {
  const LocalRhythmServerStatus({
    required this.supported,
    required this.running,
    this.pid,
    this.executablePath,
    this.dataDir,
  });

  final bool supported;
  final bool running;
  final int? pid;
  final String? executablePath;
  final String? dataDir;

  factory LocalRhythmServerStatus.fromMap(Map<dynamic, dynamic> map) {
    return LocalRhythmServerStatus(
      supported: map['supported'] as bool? ?? false,
      running: map['running'] as bool? ?? false,
      pid: (map['pid'] as num?)?.toInt(),
      executablePath: map['executablePath'] as String?,
      dataDir: map['dataDir'] as String?,
    );
  }

  static const unsupported = LocalRhythmServerStatus(
    supported: false,
    running: false,
  );
}

class LocalRhythmServerService {
  LocalRhythmServerService._();

  static final LocalRhythmServerService instance = LocalRhythmServerService._();

  static const MethodChannel _channel = MethodChannel(
    'lighting.rhythm.app/local_rhythm_server',
  );

  Future<LocalRhythmServerStatus>? _startFuture;

  bool get canManageLocalServer =>
      !kIsWeb && defaultTargetPlatform == TargetPlatform.macOS;

  bool isLocalEndpoint(String host, int port) {
    if (port != localRhythmServerDefaultPort) return false;
    final normalized = host.trim().toLowerCase();
    return normalized == localRhythmServerHost ||
        normalized == 'localhost' ||
        normalized == '::1' ||
        normalized == '0:0:0:0:0:0:0:1';
  }

  Future<LocalRhythmServerStatus> start({
    int port = localRhythmServerDefaultPort,
  }) {
    if (!canManageLocalServer) {
      return Future.value(LocalRhythmServerStatus.unsupported);
    }
    return _startFuture ??= _start(port: port).whenComplete(() {
      _startFuture = null;
    });
  }

  Future<LocalRhythmServerStatus> _start({required int port}) async {
    final raw = await _channel.invokeMapMethod<String, dynamic>(
      'start',
      {'port': port},
    );
    final status = LocalRhythmServerStatus.fromMap(raw ?? const {});
    if (!status.supported) return status;

    await _waitUntilHealthy(port);
    return status;
  }

  Future<LocalRhythmServerStatus> status() async {
    if (!canManageLocalServer) {
      return LocalRhythmServerStatus.unsupported;
    }
    final raw = await _channel.invokeMapMethod<String, dynamic>('status');
    return LocalRhythmServerStatus.fromMap(raw ?? const {});
  }

  Future<void> stop() async {
    if (!canManageLocalServer) return;
    await _channel.invokeMethod<void>('stop');
  }

  Future<void> _waitUntilHealthy(int port) async {
    final deadline = DateTime.now().add(const Duration(seconds: 10));
    Object? lastError;
    while (DateTime.now().isBefore(deadline)) {
      try {
        final healthy = await RhythmDiagnosticsApi(
          host: localRhythmServerHost,
          port: port,
        ).healthCheck().timeout(const Duration(milliseconds: 500));
        if (healthy) return;
      } catch (error) {
        lastError = error;
      }
      await Future<void>.delayed(const Duration(milliseconds: 250));
    }

    throw TimeoutException(
      'Local Rhythm Server did not become healthy'
      '${lastError == null ? '' : ': $lastError'}',
      const Duration(seconds: 10),
    );
  }
}
