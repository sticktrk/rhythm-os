import 'package:dio/dio.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';
import 'package:mocktail/mocktail.dart';

import '../helpers/mock_dio.dart';

void main() {
  late MockDio dio;
  late List<List<RhythmRoomState>> cacheUpdates;
  late RhythmServerApi api;

  setUp(() {
    dio = MockDio();
    cacheUpdates = [];
    api = RhythmServerApi(dio, onStatesReceived: (states) {
      cacheUpdates.add(states);
    });

    // Register fallback values for mocktail matchers.
    registerFallbackValue(Options());
  });

  // ---------------------------------------------------------------------------
  // roomAction
  // ---------------------------------------------------------------------------
  group('roomAction', () {
    test('parses rooms-wrapped response and invokes onStatesReceived',
        () async {
      when(() => dio.put(any(), data: any(named: 'data')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(path: 'api/rooms/action'),
                statusCode: 200,
                data: {
                  'rooms': [
                    {
                      'room_id': 'r1',
                      'rhythm_enabled': true,
                      'time_offset': 0.0,
                      'brightness_offset': 0.0,
                      'soft_off': false,
                      'lights_on': true,
                      'brightness': 80,
                      'kelvin': 4000,
                    },
                  ],
                },
              ));

      final result = await api.roomAction(roomId: 'r1', action: 'enable');

      expect(result, isNotNull);
      expect(result!.roomId, 'r1');
      expect(result.rhythmEnabled, isTrue);
      expect(result.brightness, 80);
      expect(result.kelvin, 4000);

      // Verify cache updater was called.
      expect(cacheUpdates, hasLength(1));
      expect(cacheUpdates[0], hasLength(1));
      expect(cacheUpdates[0][0].roomId, 'r1');

      verify(() => dio.put('api/rooms/action', data: {
            'room_id': 'r1',
            'action': 'enable',
          })).called(1);
    });

    test('handles backward-compat flat room object (no rooms wrapper)',
        () async {
      when(() => dio.put(any(), data: any(named: 'data')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(path: 'api/rooms/action'),
                statusCode: 200,
                data: {
                  'room_id': 'r2',
                  'rhythm_enabled': false,
                  'time_offset': 1.5,
                  'brightness_offset': 0.0,
                  'soft_off': true,
                },
              ));

      final result = await api.roomAction(roomId: 'r2', action: 'disable');

      expect(result, isNotNull);
      expect(result!.roomId, 'r2');
      expect(result.rhythmEnabled, isFalse);
      expect(result.softOff, isTrue);
      expect(cacheUpdates, hasLength(1));
    });

    test('returns null on DioException (does not throw)', () async {
      when(() => dio.put(any(), data: any(named: 'data')))
          .thenThrow(DioException(
        requestOptions: RequestOptions(path: 'api/rooms/action'),
        type: DioExceptionType.connectionTimeout,
      ));

      final result = await api.roomAction(roomId: 'r1', action: 'enable');
      expect(result, isNull);
      expect(cacheUpdates, isEmpty);
    });

    test('returns null when response has no room data', () async {
      when(() => dio.put(any(), data: any(named: 'data')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(path: 'api/rooms/action'),
                statusCode: 200,
                data: {'ok': true},
              ));

      final result = await api.roomAction(roomId: 'r1', action: 'enable');
      expect(result, isNull);
      expect(cacheUpdates, isEmpty);
    });
  });

  // ---------------------------------------------------------------------------
  // roomActionBatch
  // ---------------------------------------------------------------------------
  group('roomActionBatch', () {
    test('returns empty list for empty input without HTTP call', () async {
      final result = await api.roomActionBatch([]);

      expect(result, isEmpty);
      verifyNever(() => dio.put(any(),
          data: any(named: 'data'), options: any(named: 'options')));
    });

    test('sends array body and parses rooms response', () async {
      when(() => dio.put(any(),
              data: any(named: 'data'), options: any(named: 'options')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(path: 'api/rooms/action'),
                statusCode: 200,
                data: {
                  'rooms': [
                    {
                      'room_id': 'r1',
                      'rhythm_enabled': true,
                      'time_offset': 0.0,
                      'brightness_offset': 0.0,
                      'soft_off': false,
                    },
                    {
                      'room_id': 'r2',
                      'rhythm_enabled': false,
                      'time_offset': 0.0,
                      'brightness_offset': 0.0,
                      'soft_off': true,
                    },
                  ],
                },
              ));

      final result = await api.roomActionBatch([
        (roomId: 'r1', action: 'enable'),
        (roomId: 'r2', action: 'disable'),
      ]);

      expect(result, hasLength(2));
      expect(result[0].roomId, 'r1');
      expect(result[1].roomId, 'r2');
      expect(cacheUpdates, hasLength(1));
      expect(cacheUpdates[0], hasLength(2));
    });

    test('returns empty list on DioException', () async {
      when(() => dio.put(any(),
          data: any(named: 'data'),
          options: any(named: 'options'))).thenThrow(DioException(
        requestOptions: RequestOptions(path: 'api/rooms/action'),
      ));

      final result = await api.roomActionBatch([
        (roomId: 'r1', action: 'enable'),
      ]);
      expect(result, isEmpty);
    });
  });

  // ---------------------------------------------------------------------------
  // fixMyLights
  // ---------------------------------------------------------------------------
  group('fixMyLights', () {
    test('parses response using "rooms" key', () async {
      when(() => dio.post(any(), options: any(named: 'options')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(path: 'api/rooms/fix'),
                statusCode: 200,
                data: {
                  'rooms': [
                    {
                      'room_id': 'r1',
                      'rhythm_enabled': true,
                      'time_offset': 0.0,
                      'brightness_offset': 0.0,
                      'soft_off': false,
                      'brightness': 75,
                      'kelvin': 4200,
                    },
                  ],
                },
              ));

      final result = await api.fixMyLights();
      expect(result, hasLength(1));
      expect(result[0].roomId, 'r1');
      expect(result[0].brightness, 75);
      expect(cacheUpdates, hasLength(1));
    });

    test('parses response using "room_states" key', () async {
      when(() => dio.post(any(), options: any(named: 'options')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(path: 'api/rooms/fix'),
                statusCode: 200,
                data: {
                  'room_states': [
                    {
                      'room_id': 'r2',
                      'rhythm_enabled': false,
                      'time_offset': 0.0,
                      'brightness_offset': 0.0,
                      'soft_off': false,
                    },
                  ],
                },
              ));

      final result = await api.fixMyLights();
      expect(result, hasLength(1));
      expect(result[0].roomId, 'r2');
    });

    test('returns empty list on error', () async {
      when(() => dio.post(any(), options: any(named: 'options')))
          .thenThrow(DioException(
        requestOptions: RequestOptions(path: 'api/rooms/fix'),
      ));

      final result = await api.fixMyLights();
      expect(result, isEmpty);
    });
  });

  // ---------------------------------------------------------------------------
  // absorbTimeOffset
  // ---------------------------------------------------------------------------
  group('absorbTimeOffset', () {
    test('returns RhythmCurveConfig on success', () async {
      when(() => dio.post(
            any(),
            queryParameters: any(named: 'queryParameters'),
            data: any(named: 'data'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/config/absorb-offset'),
            statusCode: 200,
            data: {
              'id': 'rhythm',
              'name': 'Rhythm Profile',
              'curve': {
                'type': 'super-gaussian',
                'shape_p': 6.0,
              },
              'min_brightness': 2,
              'max_brightness': 100,
              'min_color_temp': 1800,
              'max_color_temp': 5500,
              'max_dim_steps': 6,
              'rhythm_interval_secs': 60,
            },
          ));

      final result = await api.absorbTimeOffset(30.0);

      expect(result, isNotNull);
      expect(result!.minBrightness, 2);
      expect(result.maxBrightness, 100);
      expect(result.minColorTemp, 1800);
      expect(result.widthLeftBri, 0.95);
      expect(result.shapeP, 6.0);
      expect(result.maxDimSteps, 6);

      final captured = verify(() => dio.post(
            'api/config/absorb-offset',
            queryParameters: captureAny(named: 'queryParameters'),
            data: captureAny(named: 'data'),
          )).captured;
      expect(captured[0], isEmpty);
      expect(captured[1], {'offset_minutes': 30.0});
    });

    test('passes the profile id when provided', () async {
      when(() => dio.post(
            any(),
            queryParameters: any(named: 'queryParameters'),
            data: any(named: 'data'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/config/absorb-offset'),
            statusCode: 200,
            data: {
              'curve': {'type': 'super-gaussian'}
            },
          ));

      await api.absorbTimeOffset(15.0, id: 'sleep');

      final captured = verify(() => dio.post(
            'api/config/absorb-offset',
            queryParameters: captureAny(named: 'queryParameters'),
            data: captureAny(named: 'data'),
          )).captured;
      expect(captured[0], {'id': 'sleep'});
      expect(captured[1], {'offset_minutes': 15.0});
    });

    test('returns null on DioException', () async {
      when(() => dio.post(
            any(),
            queryParameters: any(named: 'queryParameters'),
            data: any(named: 'data'),
          )).thenThrow(DioException(
        requestOptions: RequestOptions(path: 'api/config/absorb-offset'),
      ));

      final result = await api.absorbTimeOffset(10.0);
      expect(result, isNull);
    });
  });

  // ---------------------------------------------------------------------------
  // resetConfig
  // ---------------------------------------------------------------------------
  group('resetConfig', () {
    test('returns RhythmCurveConfig on success', () async {
      when(() =>
              dio.post(any(), queryParameters: any(named: 'queryParameters')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(path: 'api/config/reset'),
                statusCode: 200,
                data: {
                  'id': 'rhythm',
                  'name': 'Rhythm Profile',
                  'curve': {'type': 'super-gaussian'},
                  'min_brightness': 2,
                  'max_brightness': 100,
                  'min_color_temp': 1800,
                  'max_color_temp': 5500,
                  'max_dim_steps': 6,
                },
              ));

      final result = await api.resetConfig();
      expect(result, isNotNull);
      expect(result!.minColorTemp, 1800);
      final captured = verify(() => dio.post(
            'api/config/reset',
            queryParameters: captureAny(named: 'queryParameters'),
          )).captured.single as Map<String, dynamic>;
      expect(captured, isEmpty);
    });

    test('passes the profile id when provided', () async {
      when(() =>
              dio.post(any(), queryParameters: any(named: 'queryParameters')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(path: 'api/config/reset'),
                statusCode: 200,
                data: {
                  'curve': {'type': 'super-gaussian'}
                },
              ));

      await api.resetConfig(id: 'day_idle');

      final captured = verify(() => dio.post(
            'api/config/reset',
            queryParameters: captureAny(named: 'queryParameters'),
          )).captured.single as Map<String, dynamic>;
      expect(captured, {'id': 'day_idle'});
    });

    test('returns null on DioException', () async {
      when(() =>
              dio.post(any(), queryParameters: any(named: 'queryParameters')))
          .thenThrow(DioException(
        requestOptions: RequestOptions(path: 'api/config/reset'),
      ));

      final result = await api.resetConfig();
      expect(result, isNull);
    });
  });

  // ---------------------------------------------------------------------------
  // settingsSet
  // ---------------------------------------------------------------------------
  group('settingsSet', () {
    test('skips PUT when all params are null (data map empty)', () async {
      final saved = await api.settingsSet();

      expect(saved, isTrue);

      verifyNever(() => dio.put(
            any(),
            data: any(named: 'data'),
            queryParameters: any(named: 'queryParameters'),
          ));
    });

    test('sends only power_save to /api/settings', () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            queryParameters: any(named: 'queryParameters'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/settings'),
            statusCode: 200,
          ));

      final saved = await api.settingsSet(powerSave: true);

      expect(saved, isTrue);
      final captured = verify(() => dio.put(
            'api/settings',
            data: captureAny(named: 'data'),
            queryParameters: captureAny(named: 'queryParameters'),
          )).captured;
      expect(captured[0], {'power_save': true});
      expect(captured[1], isNull);
    });

    test('returns false on DioException', () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            queryParameters: any(named: 'queryParameters'),
          )).thenThrow(DioException(
        requestOptions: RequestOptions(path: 'api/settings'),
      ));

      final saved = await api.settingsSet(powerSave: true);

      expect(saved, isFalse);
    });

    test('includes null idle_profile_id when clearing custom idle', () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            queryParameters: any(named: 'queryParameters'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/mode'),
            statusCode: 200,
          ));

      await api.modeSet(
        configs: const [
          RhythmModeConfig(
            mode: RhythmMode.day,
            activeProfileId: 'rhythm',
            idleProfileId: null,
          ),
        ],
      );

      final captured = verify(() => dio.put(
            'api/mode',
            data: captureAny(named: 'data'),
            queryParameters: captureAny(named: 'queryParameters'),
          )).captured;
      expect(captured[0], {
        'configs': [
          {
            'mode': 'day',
            'active_profile_id': 'rhythm',
            'idle_profile_id': null,
            'wake_profile_id': null,
            'warning_profile_id': null,
          },
        ],
      });
    });
  });

  // ---------------------------------------------------------------------------
  // ping
  // ---------------------------------------------------------------------------
  group('ping', () {
    test('returns true when statusCode is 200', () async {
      when(() => dio.get(any())).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'health'),
            statusCode: 200,
            data: {'status': 'healthy'},
          ));

      expect(await api.ping(), isTrue);
      verify(() => dio.get('health')).called(1);
    });

    test('returns false when statusCode is not 200', () async {
      when(() => dio.get(any())).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'health'),
            statusCode: 503,
          ));

      expect(await api.ping(), isFalse);
    });

    test('returns false on DioException', () async {
      when(() => dio.get(any())).thenThrow(DioException(
        requestOptions: RequestOptions(path: 'health'),
        type: DioExceptionType.connectionTimeout,
      ));

      expect(await api.ping(), isFalse);
    });
  });

  // ---------------------------------------------------------------------------
  // Fire-and-forget methods (don't throw on DioException)
  // ---------------------------------------------------------------------------
  group('fire-and-forget methods', () {
    test('roomBrightness does not throw on DioException', () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            queryParameters: any(named: 'queryParameters'),
          )).thenThrow(DioException(
        requestOptions: RequestOptions(path: 'api/rooms/brightness'),
      ));

      // Should complete without throwing.
      await api.roomBrightness(roomId: 'r1', brightness: 50);
    });

    test('roomOffset does not throw on DioException', () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            queryParameters: any(named: 'queryParameters'),
          )).thenThrow(DioException(
        requestOptions: RequestOptions(path: 'api/rooms/offset'),
      ));

      await api.roomOffset(roomId: 'r1', timeOffset: 2.0);
    });

    test('roomPreferencesSet does not throw on DioException', () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            queryParameters: any(named: 'queryParameters'),
          )).thenThrow(DioException(
        requestOptions: RequestOptions(path: 'api/rooms/preferences'),
      ));

      await api.roomPreferencesSet(roomId: 'r1', rhythmEnabled: true);
    });

    test('configSet returns false on DioException', () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            queryParameters: any(named: 'queryParameters'),
          )).thenThrow(DioException(
        requestOptions: RequestOptions(path: 'api/config'),
      ));

      final saved = await api.configSet(
        const RhythmCurveConfig(),
        id: 'day_idle',
      );

      expect(saved, isFalse);
    });

    test('locationSet does not throw on DioException', () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            queryParameters: any(named: 'queryParameters'),
          )).thenThrow(DioException(
        requestOptions: RequestOptions(path: 'api/location'),
      ));

      await api.locationSet(lat: 40.0, lon: -90.0);
    });
  });
}
