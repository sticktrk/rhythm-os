import 'dart:async';
import 'dart:typed_data';

import 'package:dio/dio.dart';
import 'package:logging/logging.dart';

import '../models/rhythm_firmware.dart';

/// OTA firmware update service.
///
/// Stream-based alternative to the ChangeNotifier-based OtaService.
class RhythmOtaApi {
  static final _log = Logger('rhythm_sdk.ota');

  static const _manifestUrl =
      'https://dl.rhythm.lighting/esp32/manifest.json';

  /// Check for a firmware update.
  ///
  /// Returns the available release if newer than [currentVersion], or null.
  Future<RhythmFirmwareRelease?> checkForUpdate(String currentVersion) async {
    final dio = Dio(BaseOptions(
      connectTimeout: const Duration(seconds: 10),
      receiveTimeout: const Duration(seconds: 10),
    ));
    try {
      final response = await dio.get(_manifestUrl);
      final data = response.data as Map<String, dynamic>;
      final release = RhythmFirmwareRelease.fromJson(data);
      return _isNewer(release.version, currentVersion) ? release : null;
    } finally {
      dio.close();
    }
  }

  /// Start a firmware update, emitting progress events.
  ///
  /// The stream completes on success or emits an error event on failure.
  Stream<RhythmOtaProgress> startUpdate(
    RhythmFirmwareRelease release, {
    required String deviceHost,
    int port = 80,
  }) async* {
    final baseUrl =
        port != 80 ? 'http://$deviceHost:$port' : 'http://$deviceHost';

    // 1. Download firmware binary from CDN
    _log.config('OTA: downloading firmware from ${release.url}');
    yield const RhythmOtaProgress(state: RhythmOtaState.downloading);

    final Uint8List firmware;
    final downloadDio = Dio(BaseOptions(
      connectTimeout: const Duration(seconds: 15),
      receiveTimeout: const Duration(seconds: 120),
      responseType: ResponseType.bytes,
    ));
    try {
      int lastPercent = 0;
      final downloadResponse = await downloadDio.get<List<int>>(
        release.url,
        onReceiveProgress: (received, total) {
          if (total > 0) {
            lastPercent = (received * 100 ~/ total).clamp(0, 100);
          }
        },
      );
      firmware = Uint8List.fromList(downloadResponse.data!);
      yield RhythmOtaProgress(
          state: RhythmOtaState.downloading, progressPercent: lastPercent);
    } catch (e) {
      yield RhythmOtaProgress(
        state: RhythmOtaState.error,
        errorMessage: 'Failed to download firmware: $e',
      );
      return;
    } finally {
      downloadDio.close();
    }

    // 2. Upload firmware to device
    _log.config('OTA: uploading to $deviceHost');
    yield const RhythmOtaProgress(state: RhythmOtaState.uploading);

    final uploadDio = Dio(BaseOptions(
      connectTimeout: const Duration(seconds: 10),
      receiveTimeout: const Duration(seconds: 120),
    ));
    try {
      RhythmOtaState currentState = RhythmOtaState.uploading;
      final uploadResponse = await uploadDio.post<Map<String, dynamic>>(
        '$baseUrl/api/ota/upload',
        data: Stream.fromIterable([firmware]),
        options: Options(
          contentType: 'application/octet-stream',
          headers: {'Content-Length': firmware.length},
        ),
        onSendProgress: (sent, total) {
          if (total > 0) {
            final percent = (sent * 100 ~/ total).clamp(0, 100);
            if (percent >= 100 && currentState == RhythmOtaState.uploading) {
              currentState = RhythmOtaState.flashing;
            }
          }
        },
      );

      final status = uploadResponse.data?['status'] as String?;
      if (uploadResponse.statusCode == 200 && status == 'ok') {
        yield const RhythmOtaProgress(
            state: RhythmOtaState.rebooting, progressPercent: 100);
      } else {
        final message =
            uploadResponse.data?['message'] as String? ?? 'Upload failed';
        yield RhythmOtaProgress(state: RhythmOtaState.error, errorMessage: message);
        return;
      }
    } catch (e) {
      String errorMsg = 'OTA failed: $e';
      if (e is DioException && e.response?.data is Map<String, dynamic>) {
        errorMsg = (e.response!.data as Map<String, dynamic>)['message']
                as String? ??
            errorMsg;
      }
      yield RhythmOtaProgress(state: RhythmOtaState.error, errorMessage: errorMsg);
      return;
    } finally {
      uploadDio.close();
    }

    // 3. Wait for reboot and verify version
    _log.config('OTA: rebooting, waiting for version verification');
    await Future.delayed(const Duration(seconds: 8));

    final deadline = DateTime.now().add(const Duration(seconds: 30));
    final versionDio = Dio(BaseOptions(
      connectTimeout: const Duration(seconds: 3),
      receiveTimeout: const Duration(seconds: 3),
    ));
    try {
      while (DateTime.now().isBefore(deadline)) {
        try {
          final response = await versionDio.get('$baseUrl/api/ota/version');
          final version = response.data['version'] as String?;
          if (version == release.version) {
            yield const RhythmOtaProgress(
                state: RhythmOtaState.complete, progressPercent: 100);
            return;
          }
        } catch (e) {
          _log.fine('OTA version check pending', e);
        }
        await Future.delayed(const Duration(seconds: 2));
      }
      yield const RhythmOtaProgress(
        state: RhythmOtaState.error,
        errorMessage: 'Device did not come back online after update',
      );
    } finally {
      versionDio.close();
    }
  }

  /// Compare semver strings. Returns true if [remote] is newer than [local].
  bool _isNewer(String remote, String local) {
    final rParts =
        remote.split('.').map((s) => int.tryParse(s) ?? 0).toList();
    final lParts =
        local.split('.').map((s) => int.tryParse(s) ?? 0).toList();
    while (rParts.length < 3) {
      rParts.add(0);
    }
    while (lParts.length < 3) {
      lParts.add(0);
    }
    for (int i = 0; i < 3; i++) {
      if (rParts[i] > lParts[i]) return true;
      if (rParts[i] < lParts[i]) return false;
    }
    return false;
  }
}
