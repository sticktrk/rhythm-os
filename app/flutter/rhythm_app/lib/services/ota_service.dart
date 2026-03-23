/// OTA firmware update service for ESP32.
///
/// Manages the lifecycle of firmware updates:
/// 1. Check for updates from dl.rhythm.lighting
/// 2. Download the firmware binary in the app
/// 3. Push it to the ESP32 via HTTP POST /api/ota/upload
/// 4. Wait for reboot and verify new version
library;

import 'dart:async';
import 'dart:typed_data';

import 'package:dio/dio.dart';
import 'package:flutter/foundation.dart';

/// OTA update state machine.
enum OtaState {
  idle,
  checking,
  available,
  upToDate,
  downloading,
  uploading,
  /// Bytes sent — waiting for ESP32 to finish writing firmware to flash.
  flashing,
  rebooting,
  complete,
  error,
}

/// Firmware release metadata from the manifest.
class FirmwareRelease {
  final String version;
  final String url;
  final int? size;
  final String? changelog;

  const FirmwareRelease({
    required this.version,
    required this.url,
    this.size,
    this.changelog,
  });

  factory FirmwareRelease.fromJson(Map<String, dynamic> json) {
    final relativeUrl = json['url'] as String? ?? '';
    return FirmwareRelease(
      version: json['version'] as String? ?? '0.0.0',
      url: 'https://dl.rhythm.lighting/esp32/$relativeUrl',
      size: json['size'] as int?,
      changelog: json['changelog'] as String?,
    );
  }
}

/// OTA firmware update service.
///
/// Extends [ChangeNotifier] so the settings UI can react to state changes.
class OtaService extends ChangeNotifier {
  static const _manifestUrl =
      'https://dl.rhythm.lighting/esp32/manifest.json';

  final Dio _dio;

  OtaState _state = OtaState.idle;
  FirmwareRelease? _availableRelease;
  int _progress = 0;
  String? _errorMessage;

  OtaService()
      : _dio = Dio(BaseOptions(
          connectTimeout: const Duration(seconds: 10),
          receiveTimeout: const Duration(seconds: 10),
        ));

  OtaState get state => _state;
  FirmwareRelease? get availableRelease => _availableRelease;
  int get progress => _progress;
  String? get errorMessage => _errorMessage;

  /// Check for a firmware update.
  ///
  /// Fetches the manifest from dl.rhythm.lighting and compares the version
  /// against [currentVersion]. Updates state to [OtaState.available] or
  /// [OtaState.upToDate].
  Future<void> checkForUpdate(String currentVersion) async {
    _state = OtaState.checking;
    _errorMessage = null;
    notifyListeners();

    try {
      final response = await _dio.get(_manifestUrl);
      final data = response.data as Map<String, dynamic>;
      final release = FirmwareRelease.fromJson(data);

      if (_isNewer(release.version, currentVersion)) {
        _availableRelease = release;
        _state = OtaState.available;
      } else {
        _availableRelease = null;
        _state = OtaState.upToDate;
      }
    } catch (e) {
      _state = OtaState.error;
      _errorMessage = 'Failed to check for updates: $e';
    }

    notifyListeners();
  }

  /// Start the firmware update.
  ///
  /// Downloads the firmware binary from dl.rhythm.lighting, then pushes it
  /// to the ESP32 via HTTP POST. Progress tracks the upload to the device.
  Future<void> startUpdate(String deviceIp, {int port = 80}) async {
    final release = _availableRelease;
    if (release == null) return;

    final baseUrl = port != 80 ? 'http://$deviceIp:$port' : 'http://$deviceIp';

    _state = OtaState.downloading;
    _progress = 0;
    _errorMessage = null;
    notifyListeners();

    try {
      // 1. Download firmware binary from CDN
      final downloadDio = Dio(BaseOptions(
        connectTimeout: const Duration(seconds: 15),
        receiveTimeout: const Duration(seconds: 120),
        responseType: ResponseType.bytes,
      ));

      final Uint8List firmware;
      try {
        final downloadResponse = await downloadDio.get<List<int>>(
          release.url,
          onReceiveProgress: (received, total) {
            if (total > 0) {
              _progress = (received * 100 ~/ total).clamp(0, 100);
              notifyListeners();
            }
          },
        );
        firmware = Uint8List.fromList(downloadResponse.data!);
      } finally {
        downloadDio.close();
      }

      debugPrint('OTA: Downloaded ${firmware.length} bytes');

      // 2. Upload firmware to ESP32 via HTTP POST
      _state = OtaState.uploading;
      _progress = 0;
      notifyListeners();

      final uploadDio = Dio(BaseOptions(
        connectTimeout: const Duration(seconds: 10),
        // OTA flash write can be slow — allow plenty of time
        receiveTimeout: const Duration(seconds: 120),
      ));

      try {
        final uploadResponse = await uploadDio.post<Map<String, dynamic>>(
          '$baseUrl/api/ota/upload',
          data: Stream.fromIterable([firmware]),
          options: Options(
            contentType: 'application/octet-stream',
            headers: {
              'Content-Length': firmware.length,
            },
          ),
          onSendProgress: (sent, total) {
            if (total > 0) {
              _progress = (sent * 100 ~/ total).clamp(0, 100);
              // Bytes are buffered to TCP fast, but ESP32 is still writing
              // to flash. Switch to indeterminate "flashing" state.
              if (_progress >= 100 && _state == OtaState.uploading) {
                _state = OtaState.flashing;
              }
              notifyListeners();
            }
          },
        );

        final status = uploadResponse.data?['status'] as String?;
        if (uploadResponse.statusCode == 200 && status == 'ok') {
          _state = OtaState.rebooting;
          _progress = 100;
          notifyListeners();
          await _waitForReboot(baseUrl, release.version);
        } else {
          final message =
              uploadResponse.data?['message'] as String? ?? 'Upload failed';
          _state = OtaState.error;
          _errorMessage = message;
          notifyListeners();
        }
      } finally {
        uploadDio.close();
      }
    } on DioException catch (e) {
      _state = OtaState.error;
      if (e.response != null) {
        final data = e.response?.data;
        if (data is Map<String, dynamic>) {
          _errorMessage = data['message'] as String? ?? 'OTA failed: ${e.message}';
        } else {
          _errorMessage = 'OTA failed: ${e.message}';
        }
      } else {
        _errorMessage = 'OTA failed: ${e.message}';
      }
      notifyListeners();
    } catch (e) {
      _state = OtaState.error;
      _errorMessage = 'OTA failed: $e';
      notifyListeners();
    }
  }

  /// Wait for the ESP32 to reboot and verify the new version.
  Future<void> _waitForReboot(String baseUrl, String expectedVersion) async {
    // Give the device time to reboot
    await Future.delayed(const Duration(seconds: 8));

    // Poll the version endpoint for up to 30 seconds
    final deadline = DateTime.now().add(const Duration(seconds: 30));
    final versionDio = Dio(BaseOptions(
      connectTimeout: const Duration(seconds: 3),
      receiveTimeout: const Duration(seconds: 3),
    ));

    try {
      while (DateTime.now().isBefore(deadline)) {
        try {
          final response =
              await versionDio.get('$baseUrl/api/ota/version');
          final version = response.data['version'] as String?;

          if (version == expectedVersion) {
            _state = OtaState.complete;
            notifyListeners();
            return;
          }
        } catch (_) {
          // Device still rebooting, retry
        }
        await Future.delayed(const Duration(seconds: 2));
      }

      // Timed out waiting for reboot
      _state = OtaState.error;
      _errorMessage = 'Device did not come back online after update';
      notifyListeners();
    } finally {
      versionDio.close();
    }
  }

  /// Reset to idle state (e.g., to retry or dismiss).
  void reset() {
    _state = OtaState.idle;
    _availableRelease = null;
    _progress = 0;
    _errorMessage = null;
    notifyListeners();
  }

  /// Compare semver strings. Returns true if [remote] is newer than [local].
  bool _isNewer(String remote, String local) {
    final rParts = remote.split('.').map((s) => int.tryParse(s) ?? 0).toList();
    final lParts = local.split('.').map((s) => int.tryParse(s) ?? 0).toList();

    // Pad to 3 parts
    while (rParts.length < 3) rParts.add(0);
    while (lParts.length < 3) lParts.add(0);

    for (int i = 0; i < 3; i++) {
      if (rParts[i] > lParts[i]) return true;
      if (rParts[i] < lParts[i]) return false;
    }
    return false;
  }
}
