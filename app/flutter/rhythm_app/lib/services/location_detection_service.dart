import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:geolocator/geolocator.dart';

class DetectedPosition {
  final Position position;
  final bool fromCache;

  const DetectedPosition({
    required this.position,
    required this.fromCache,
  });
}

class LocationDetectionService {
  static bool isGranted(LocationPermission permission) {
    return permission == LocationPermission.whileInUse ||
        permission == LocationPermission.always;
  }

  static Future<LocationPermission> requestPermissionIfNeeded({
    required String debugSource,
  }) async {
    final before = await Geolocator.checkPermission();
    debugPrint('$debugSource: location permission before request: $before');

    if (isGranted(before) || before == LocationPermission.deniedForever) {
      return before;
    }

    final requested = await Geolocator.requestPermission();
    debugPrint('$debugSource: location permission request result: $requested');

    final after = await Geolocator.checkPermission();
    debugPrint('$debugSource: location permission after request: $after');

    return isGranted(after) ? after : requested;
  }

  static Future<DetectedPosition> getCurrentPosition({
    required String debugSource,
    Duration timeLimit = const Duration(seconds: 15),
    bool allowCachedFallback = true,
  }) async {
    try {
      final position = await Geolocator.getCurrentPosition(
        locationSettings: _locationSettings(
          timeLimit,
          forceAndroidLocationManager: false,
        ),
      );
      _logPosition(debugSource, position, fromCache: false);
      return DetectedPosition(position: position, fromCache: false);
    } on TimeoutException catch (error, stackTrace) {
      return _fallbackOrRethrow(
        debugSource: debugSource,
        error: error,
        stackTrace: stackTrace,
        allowCachedFallback: allowCachedFallback,
      );
    } on LocationServiceDisabledException {
      rethrow;
    } on PermissionDeniedException {
      rethrow;
    } catch (error, stackTrace) {
      return _fallbackOrRethrow(
        debugSource: debugSource,
        error: error,
        stackTrace: stackTrace,
        allowCachedFallback: allowCachedFallback,
      );
    }
  }

  static LocationSettings _locationSettings(
    Duration timeLimit, {
    required bool forceAndroidLocationManager,
  }) {
    if (defaultTargetPlatform == TargetPlatform.android) {
      return AndroidSettings(
        accuracy: LocationAccuracy.high,
        forceLocationManager: forceAndroidLocationManager,
        intervalDuration: const Duration(seconds: 1),
        timeLimit: timeLimit,
      );
    }

    return LocationSettings(
      accuracy: LocationAccuracy.low,
      timeLimit: timeLimit,
    );
  }

  static Future<DetectedPosition> _fallbackOrRethrow({
    required String debugSource,
    required Object error,
    required StackTrace stackTrace,
    required bool allowCachedFallback,
  }) async {
    debugPrint('$debugSource: current position failed: $error');

    if (allowCachedFallback) {
      try {
        final cached = await Geolocator.getLastKnownPosition();
        if (cached != null) {
          _logPosition(debugSource, cached, fromCache: true);
          return DetectedPosition(position: cached, fromCache: true);
        }
        debugPrint('$debugSource: no fused last known position available');
      } catch (fallbackError) {
        debugPrint('$debugSource: last known position failed: $fallbackError');
      }
    }

    if (defaultTargetPlatform == TargetPlatform.android) {
      if (allowCachedFallback) {
        try {
          final cached = await Geolocator.getLastKnownPosition(
            forceAndroidLocationManager: true,
          );
          if (cached != null) {
            _logPosition(debugSource, cached, fromCache: true);
            return DetectedPosition(position: cached, fromCache: true);
          }
          debugPrint(
            '$debugSource: no location-manager last known position available',
          );
        } catch (fallbackError) {
          debugPrint(
            '$debugSource: location-manager last known position failed: '
            '$fallbackError',
          );
        }
      }

      try {
        debugPrint(
          '$debugSource: retrying current position with Android LocationManager',
        );
        final position = await Geolocator.getCurrentPosition(
          locationSettings: _locationSettings(
            const Duration(seconds: 10),
            forceAndroidLocationManager: true,
          ),
        );
        _logPosition(debugSource, position, fromCache: false);
        return DetectedPosition(position: position, fromCache: false);
      } catch (retryError) {
        debugPrint(
          '$debugSource: location-manager current position failed: '
          '$retryError',
        );
      }
    }

    Error.throwWithStackTrace(error, stackTrace);
  }

  static void _logPosition(
    String debugSource,
    Position position, {
    required bool fromCache,
  }) {
    debugPrint(
      '$debugSource: ${fromCache ? 'cached' : 'current'} position '
      'lat=${position.latitude.toStringAsFixed(5)} '
      'lon=${position.longitude.toStringAsFixed(5)} '
      'accuracy=${position.accuracy.toStringAsFixed(1)}m',
    );
  }
}
