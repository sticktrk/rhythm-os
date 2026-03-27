/// OTA firmware update service for ESP32.
///
/// Wraps [RhythmOtaApi] from the SDK with a [ChangeNotifier] surface
/// so the settings UI can react to state changes.
library;

import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' as sdk;

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

  factory FirmwareRelease._fromSdk(sdk.RhythmFirmwareRelease r) =>
      FirmwareRelease(
        version: r.version,
        url: r.url,
        size: r.size,
        changelog: r.changelog,
      );
}

/// OTA firmware update service.
///
/// Extends [ChangeNotifier] so the settings UI can react to state changes.
/// Delegates all HTTP logic to [sdk.RhythmOtaApi].
class OtaService extends ChangeNotifier {
  final sdk.RhythmOtaApi _api = sdk.RhythmOtaApi();

  OtaState _state = OtaState.idle;
  FirmwareRelease? _availableRelease;
  sdk.RhythmFirmwareRelease? _sdkRelease;
  int _progress = 0;
  String? _errorMessage;
  StreamSubscription<sdk.RhythmOtaProgress>? _updateSub;

  OtaState get state => _state;
  FirmwareRelease? get availableRelease => _availableRelease;
  int get progress => _progress;
  String? get errorMessage => _errorMessage;

  /// Check for a firmware update.
  Future<void> checkForUpdate(String currentVersion) async {
    _state = OtaState.checking;
    _errorMessage = null;
    notifyListeners();

    try {
      final release = await _api.checkForUpdate(currentVersion);
      if (release != null) {
        _sdkRelease = release;
        _availableRelease = FirmwareRelease._fromSdk(release);
        _state = OtaState.available;
      } else {
        _sdkRelease = null;
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
  Future<void> startUpdate(String deviceIp, {int port = 80}) async {
    final release = _sdkRelease;
    if (release == null) return;

    _state = OtaState.downloading;
    _progress = 0;
    _errorMessage = null;
    notifyListeners();

    _updateSub?.cancel();
    _updateSub = _api
        .startUpdate(release, deviceHost: deviceIp, port: port)
        .listen(_onProgress);
  }

  void _onProgress(sdk.RhythmOtaProgress event) {
    _state = _mapState(event.state);
    _progress = event.progressPercent ?? _progress;
    _errorMessage = event.errorMessage;
    notifyListeners();
  }

  static OtaState _mapState(sdk.RhythmOtaState s) => switch (s) {
    sdk.RhythmOtaState.idle => OtaState.idle,
    sdk.RhythmOtaState.checking => OtaState.checking,
    sdk.RhythmOtaState.available => OtaState.available,
    sdk.RhythmOtaState.upToDate => OtaState.upToDate,
    sdk.RhythmOtaState.downloading => OtaState.downloading,
    sdk.RhythmOtaState.uploading => OtaState.uploading,
    sdk.RhythmOtaState.flashing => OtaState.flashing,
    sdk.RhythmOtaState.rebooting => OtaState.rebooting,
    sdk.RhythmOtaState.complete => OtaState.complete,
    sdk.RhythmOtaState.error => OtaState.error,
  };

  /// Reset to idle state.
  void reset() {
    _updateSub?.cancel();
    _updateSub = null;
    _state = OtaState.idle;
    _availableRelease = null;
    _sdkRelease = null;
    _progress = 0;
    _errorMessage = null;
    notifyListeners();
  }

  @override
  void dispose() {
    _updateSub?.cancel();
    super.dispose();
  }
}
