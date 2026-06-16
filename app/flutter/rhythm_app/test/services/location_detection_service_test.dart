import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:geolocator/geolocator.dart';
import 'package:rhythm_app/services/location_detection_service.dart';

void main() {
  late GeolocatorPlatform originalPlatform;

  setUp(() {
    originalPlatform = GeolocatorPlatform.instance;
  });

  tearDown(() {
    GeolocatorPlatform.instance = originalPlatform;
    debugDefaultTargetPlatformOverride = null;
  });

  test('uses permission status checked after the Android request result',
      () async {
    final geolocator = _FakeGeolocatorPlatform(
      checkResults: [
        LocationPermission.denied,
        LocationPermission.whileInUse,
      ],
      requestResult: LocationPermission.denied,
    );
    GeolocatorPlatform.instance = geolocator;

    final permission = await LocationDetectionService.requestPermissionIfNeeded(
      debugSource: 'LocationDetectionTest',
    );

    expect(permission, LocationPermission.whileInUse);
    expect(geolocator.requestPermissionCalls, 1);
  });

  test('falls back to the last known position when current lookup times out',
      () async {
    final cachedPosition = _position(latitude: 35.0, longitude: -78.9);
    GeolocatorPlatform.instance = _FakeGeolocatorPlatform(
      currentPositionError: TimeoutException('no fix'),
      lastKnownPosition: cachedPosition,
    );

    final detected = await LocationDetectionService.getCurrentPosition(
      debugSource: 'LocationDetectionTest',
    );

    expect(detected.position, cachedPosition);
    expect(detected.fromCache, isTrue);
  });

  test('retries Android current lookup through LocationManager', () async {
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    final geolocator = _FakeGeolocatorPlatform(
      currentPositionError: TimeoutException('fused timeout'),
      currentPositionFailures: 1,
    );
    GeolocatorPlatform.instance = geolocator;

    final detected = await LocationDetectionService.getCurrentPosition(
      debugSource: 'LocationDetectionTest',
    );

    expect(detected.fromCache, isFalse);
    expect(geolocator.currentPositionCalls, 2);
    expect(
      geolocator.locationSettingsLog.last,
      isA<AndroidSettings>().having(
        (settings) => settings.forceLocationManager,
        'forceLocationManager',
        isTrue,
      ),
    );
  });
}

class _FakeGeolocatorPlatform extends GeolocatorPlatform {
  _FakeGeolocatorPlatform({
    List<LocationPermission>? checkResults,
    this.requestResult = LocationPermission.whileInUse,
    this.currentPositionError,
    this.currentPositionFailures,
    this.lastKnownPosition,
  }) : _checkResults = List.of(
          checkResults ?? [LocationPermission.whileInUse],
        );

  final List<LocationPermission> _checkResults;
  final LocationPermission requestResult;
  final Object? currentPositionError;
  final int? currentPositionFailures;
  final Position? lastKnownPosition;
  int requestPermissionCalls = 0;
  int currentPositionCalls = 0;
  final locationSettingsLog = <LocationSettings?>[];

  @override
  Future<LocationPermission> checkPermission() async {
    if (_checkResults.length > 1) {
      return _checkResults.removeAt(0);
    }
    return _checkResults.first;
  }

  @override
  Future<LocationPermission> requestPermission() async {
    requestPermissionCalls++;
    return requestResult;
  }

  @override
  Future<Position> getCurrentPosition({
    LocationSettings? locationSettings,
  }) async {
    currentPositionCalls++;
    locationSettingsLog.add(locationSettings);
    final error = currentPositionError;
    final failures = currentPositionFailures ?? (error == null ? 0 : 1 << 30);
    if (error != null && currentPositionCalls <= failures) {
      throw error;
    }
    return _position(latitude: 40.0, longitude: -74.0);
  }

  @override
  Future<Position?> getLastKnownPosition({
    bool forceLocationManager = false,
  }) async {
    return lastKnownPosition;
  }
}

Position _position({
  required double latitude,
  required double longitude,
}) {
  return Position(
    latitude: latitude,
    longitude: longitude,
    timestamp: DateTime(2026),
    accuracy: 12,
    altitude: 0,
    altitudeAccuracy: 0,
    heading: 0,
    headingAccuracy: 0,
    speed: 0,
    speedAccuracy: 0,
  );
}
