import 'package:dio/dio.dart';
import 'package:mocktail/mocktail.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

import '../helpers/mock_dio.dart';

void main() {
  late MockDio dio;
  late RhythmConfigApi api;

  void stubActiveRhythmProfile() {
    when(() => dio.get('api/state')).thenAnswer(
      (_) async => Response(
        requestOptions: RequestOptions(path: 'api/state'),
        statusCode: 200,
        data: {
          'settings': {
            'power_save': false,
          },
          'mode': {
            'active': 'day',
            'configs': [
              {
                'mode': 'day',
                'active_profile_id': 'rhythm',
                'idle_profile_id': 'day_idle',
              },
              {
                'mode': 'sleep',
                'active_profile_id': 'sleep',
                'idle_profile_id': 'sleep_idle',
              },
            ],
          },
          'profiles': [
            {
              'id': 'day_idle',
              'name': 'Day Idle',
              'curve': {'type': 'inherit-active'},
              'min_brightness': 1,
              'max_brightness': 1,
              'min_color_temp': 0,
              'max_color_temp': 0,
              'max_dim_steps': 1,
              'rhythm_interval_secs': 60,
            },
            {
              'id': 'rhythm',
              'name': 'Rhythm Profile',
              'curve': {'type': 'super-gaussian'},
              'min_brightness': 2,
              'max_brightness': 100,
              'min_color_temp': 1800,
              'max_color_temp': 5500,
              'max_dim_steps': 12,
              'rhythm_interval_secs': 60,
            },
            {
              'id': 'sleep',
              'name': 'Sleep Profile',
              'curve': {'type': 'super-gaussian'},
              'min_brightness': 10,
              'max_brightness': 40,
              'min_color_temp': 500,
              'max_color_temp': 500,
              'max_dim_steps': 12,
              'rhythm_interval_secs': 60,
            },
          ],
          'active_profile': {
            'id': 'rhythm',
            'name': 'Rhythm Profile',
            'curve': {'type': 'super-gaussian'},
          },
        },
      ),
    );
    when(
      () => dio.get(
        'api/config',
        queryParameters: any(named: 'queryParameters'),
      ),
    ).thenAnswer(
      (_) async => Response(
        requestOptions: RequestOptions(path: 'api/config'),
        statusCode: 200,
        data: {
          'id': 'rhythm',
          'name': 'Rhythm Profile',
          'curve': {
            'type': 'super-gaussian',
            'width_left_bri': 0.95,
            'width_right_bri': 0.85,
            'width_left_cct': 0.95,
            'width_right_cct': 1.15,
            'shape_p': 6.0,
          },
          'min_brightness': 2,
          'max_brightness': 100,
          'min_color_temp': 1800,
          'max_color_temp': 5500,
          'max_dim_steps': 12,
          'rhythm_interval_secs': 60,
        },
      ),
    );
  }

  setUp(() {
    dio = MockDio();
    api = RhythmConfigApi(baseUrl: 'http://test/', dio: dio);
  });

  group('getConfigState', () {
    test('parses the active profile config from /api/state', () async {
      when(() => dio.get('api/state')).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/state'),
          statusCode: 200,
          data: {
            'active_profile': {
              'id': 'rhythm',
              'name': 'Rhythm Profile',
              'curve': {
                'type': 'super-gaussian',
                'width_left_bri': 0.9,
                'width_right_bri': 0.8,
                'width_left_cct': 0.95,
                'width_right_cct': 1.1,
                'shape_p': 5.0,
              },
              'min_color_temp': 2000,
              'max_color_temp': 6000,
              'min_brightness': 5,
              'max_brightness': 100,
              'max_dim_steps': 4,
            },
            'location': {
              'lat': 40.7128,
              'lon': -74.0060,
              'timezone_name': 'America/New_York',
            },
          },
        ),
      );

      final state = await api.getConfigState();

      expect(state.config.minColorTemp, 2000);
      expect(state.config.maxColorTemp, 6000);
      expect(state.config.minBrightness, 5);
      expect(state.config.maxBrightness, 100);
      expect(state.config.widthLeftBri, 0.9);
      expect(state.config.widthRightBri, 0.8);
      expect(state.config.widthLeftCct, 0.95);
      expect(state.config.widthRightCct, 1.1);
      expect(state.config.shapeP, 5.0);
      expect(state.config.maxDimSteps, 4);
      expect(state.latitude, 40.7128);
      expect(state.longitude, -74.0060);
      expect(state.timezone, 'America/New_York');
      verify(() => dio.get('api/state')).called(1);
    });

    test('throws RhythmApiException on DioException', () async {
      when(() => dio.get('api/state')).thenThrow(
        DioException(
          requestOptions: RequestOptions(path: 'api/state'),
          response: Response(
            requestOptions: RequestOptions(path: 'api/state'),
            statusCode: 500,
          ),
        ),
      );

      expect(
        () => api.getConfigState(),
        throwsA(
          isA<RhythmApiException>()
              .having((e) => e.statusCode, 'statusCode', 500),
        ),
      );
    });

    test('parses nested active_profile config/effective payloads', () async {
      when(() => dio.get('api/state')).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/state'),
          statusCode: 200,
          data: {
            'active_profile': {
              'config': {
                'id': 'rhythm',
                'name': 'Day',
                'curve': {
                  'type': 'super-gaussian',
                  'shape_p': 6.0,
                },
                'min_color_temp': 1800,
                'max_color_temp': 5500,
                'min_brightness': 2,
                'max_brightness': 100,
                'max_dim_steps': 6,
                'fade_ms': {'mode': 'auto'},
                'motion_timeout_secs': {'mode': 'auto'},
                'rhythm_interval_secs': {'mode': 'auto'},
              },
              'effective': {
                'fade_ms': 500,
                'motion_timeout_secs': 1200,
                'rhythm_interval_secs': 60,
              },
            },
          },
        ),
      );

      final state = await api.getConfigState();

      expect(state.config.fadeMs, 500);
      expect(state.config.motionTimeoutSecs, 1200);
      verify(() => dio.get('api/state')).called(1);
    });
  });

  group('saveConfig', () {
    test('preserves a constant active profile when merging raw config',
        () async {
      when(() => dio.get('api/state')).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/state'),
          statusCode: 200,
          data: {
            'settings': {
              'power_save': false,
            },
            'mode': {
              'active': 'sleep',
              'configs': [
                {
                  'mode': 'day',
                  'active_profile_id': 'rhythm',
                  'idle_profile_id': 'day_idle',
                },
                {
                  'mode': 'sleep',
                  'active_profile_id': 'sleep',
                  'idle_profile_id': 'sleep_idle',
                },
              ],
            },
            'profiles': [
              {
                'id': 'rhythm',
                'name': 'Rhythm Profile',
                'curve': {'type': 'super-gaussian'},
              },
              {
                'id': 'sleep',
                'name': 'Sleep Profile',
                'curve': {
                  'type': 'constant',
                  'brightness': 1,
                  'color_temp': 0,
                  'direct_color': {
                    'rgb': {'r': 255, 'g': 147, 'b': 41},
                    'xy': {'x': 0.675, 'y': 0.322},
                  },
                },
                'min_brightness': 10,
                'max_brightness': 40,
                'min_color_temp': 500,
                'max_color_temp': 500,
                'max_dim_steps': 12,
                'rhythm_interval_secs': 60,
              },
            ],
            'active_profile': {
              'id': 'sleep',
              'name': 'Sleep Profile',
              'curve': {
                'type': 'constant',
                'brightness': 1,
                'color_temp': 0,
                'direct_color': {
                  'rgb': {'r': 255, 'g': 147, 'b': 41},
                  'xy': {'x': 0.675, 'y': 0.322},
                },
              },
            },
          },
        ),
      );
      when(
        () => dio.get(
          'api/config',
          queryParameters: any(named: 'queryParameters'),
        ),
      ).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/config'),
          statusCode: 200,
          data: {
            'id': 'sleep',
            'name': 'Sleep Profile',
            'curve': {
              'type': 'constant',
              'brightness': 1,
              'color_temp': 0,
              'direct_color': {
                'rgb': {'r': 255, 'g': 147, 'b': 41},
                'xy': {'x': 0.675, 'y': 0.322},
              },
            },
            'min_brightness': 10,
            'max_brightness': 40,
            'min_color_temp': 500,
            'max_color_temp': 500,
            'max_dim_steps': 12,
            'rhythm_interval_secs': 60,
          },
        ),
      );
      when(
        () => dio.put(
          'api/config',
          queryParameters: any(named: 'queryParameters'),
          data: any(named: 'data'),
        ),
      ).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/config'),
          statusCode: 204,
        ),
      );

      await api.saveConfig(
        const RhythmRawConfig(
          minColorTemp: 1800,
          maxColorTemp: 5500,
          minBrightness: 2,
          maxBrightness: 95,
          widthLeftBri: 0.91,
          widthRightBri: 0.84,
          widthLeftCct: 0.93,
          widthRightCct: 1.12,
          shapeP: 6.5,
          maxDimSteps: 6,
          fadeMs: 400,
          motionTimeoutSecs: 900,
        ),
      );

      final captured = verify(
        () => dio.put(
          'api/config',
          queryParameters: captureAny(named: 'queryParameters'),
          data: captureAny(named: 'data'),
        ),
      ).captured;
      final query = captured[0] as Map<String, dynamic>;
      final body = captured[1] as Map<String, dynamic>;
      final curve = body['curve'] as Map<String, dynamic>;

      expect(query['id'], 'sleep');
      expect(body['id'], 'sleep');
      expect(body['name'], 'Sleep Profile');
      expect(body['min_brightness'], 2);
      expect(body['max_brightness'], 95);
      expect(body['min_color_temp'], 1800);
      expect(body['max_color_temp'], 5500);
      expect(body['max_dim_steps'], 6);
      expect(body['fade_ms'], {'mode': 'fixed', 'value': 400});
      expect(body['motion_timeout_secs'], {'mode': 'fixed', 'value': 900});
      expect(body['rhythm_interval_secs'], {'mode': 'fixed', 'value': 60});
      expect(curve['type'], 'constant');
      expect(curve['brightness'], 1);
      expect(curve['color_temp'], 0);
      expect(curve['direct_color'], isNotNull);
      expect(curve.containsKey('width_left_bri'), isFalse);
    });

    test('throws RhythmApiException on DioException', () async {
      when(() => dio.get('api/state')).thenThrow(
        DioException(
          requestOptions: RequestOptions(path: 'api/state'),
          response: Response(
            requestOptions: RequestOptions(path: 'api/state'),
            statusCode: 400,
          ),
        ),
      );

      expect(
        () => api.saveConfig(RhythmRawConfig.defaults()),
        throwsA(
          isA<RhythmApiException>()
              .having((e) => e.statusCode, 'statusCode', 400),
        ),
      );
    });
  });

  group('getCurveData', () {
    final previewJson = {
      'config': {
        'id': 'rhythm',
        'name': 'Rhythm Profile',
        'curve': {'type': 'super-gaussian'},
        'min_brightness': 2,
        'max_brightness': 100,
        'min_color_temp': 1800,
        'max_color_temp': 5500,
        'max_dim_steps': 12,
      },
      'solar': {
        'sunrise': 6.5,
        'sunset': 19.5,
        'solar_noon': 13.0,
        'solar_midnight': 1.0,
        'day_length': 13.0,
      },
      'curve': {
        'hours': [0.0, 6.0, 12.0, 18.0, 24.0],
        'brightness': [2, 50, 100, 50, 2],
        'kelvin': [1800, 3500, 5500, 3500, 1800],
      },
      'steps': {
        'start_hour': 12.0,
        'step_up': const [],
        'step_down': const [],
      },
    };

    test('requests stored preview for the active profile', () async {
      stubActiveRhythmProfile();
      when(
        () => dio.get(
          'api/curve',
          queryParameters: any(named: 'queryParameters'),
        ),
      ).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/curve'),
          statusCode: 200,
          data: previewJson,
        ),
      );

      final data = await api.getCurveData();

      expect(data.hours, [0.0, 6.0, 12.0, 18.0, 24.0]);
      expect(data.brightness, [2, 50, 100, 50, 2]);
      expect(data.kelvin, [1800, 3500, 5500, 3500, 1800]);
      expect(data.solar.solarNoon, 13.0);

      final query = verify(
        () => dio.get(
          'api/curve',
          queryParameters: captureAny(named: 'queryParameters'),
        ),
      ).captured.single as Map<String, dynamic>;
      expect(query['id'], 'rhythm');
      expect(query['samples_per_hour'], 4);
      expect(query['start_hour'], 12.0);
      expect(query['max_steps'], 12);
      expect(query['date'], matches(RegExp(r'^\d{4}-\d{2}-\d{2}$')));
    });

    test('posts a draft preview when overrides are supplied', () async {
      stubActiveRhythmProfile();
      when(
        () => dio.post(
          'api/curve',
          queryParameters: any(named: 'queryParameters'),
          data: any(named: 'data'),
        ),
      ).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/curve'),
          statusCode: 200,
          data: previewJson,
        ),
      );

      await api.getCurveData(
        month: 6,
        overrides: const RhythmCurveConfig(
          minBrightness: 10,
          maxBrightness: 90,
          minColorTemp: 2200,
          maxColorTemp: 6000,
          widthLeftBri: 0.92,
          widthRightBri: 0.82,
          widthLeftCct: 0.94,
          widthRightCct: 1.08,
          shapeP: 8.0,
          maxDimSteps: 4,
        ),
      );

      final captured = verify(
        () => dio.post(
          'api/curve',
          queryParameters: captureAny(named: 'queryParameters'),
          data: captureAny(named: 'data'),
        ),
      ).captured;
      final query = captured[0] as Map<String, dynamic>;
      final body = captured[1] as Map<String, dynamic>;
      final curve = body['curve'] as Map<String, dynamic>;

      expect(query['id'], 'rhythm');
      expect(query['samples_per_hour'], 4);
      expect(query['start_hour'], 12.0);
      expect(query['max_steps'], 4);
      expect(query['date'], matches(RegExp(r'^\d{4}-06-15$')));
      expect(body['min_brightness'], 10);
      expect(body['max_brightness'], 90);
      expect(body['min_color_temp'], 2200);
      expect(body['max_color_temp'], 6000);
      expect(curve['type'], 'super-gaussian');
      expect(curve['shape_p'], 8.0);
    });

    test('posts a constant draft preview when constant overrides are supplied',
        () async {
      stubActiveRhythmProfile();
      when(
        () => dio.post(
          'api/curve',
          queryParameters: any(named: 'queryParameters'),
          data: any(named: 'data'),
        ),
      ).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/curve'),
          statusCode: 200,
          data: previewJson,
        ),
      );

      await api.getCurveData(
        overrides: RhythmCurveConfig(
          minBrightness: 1,
          maxBrightness: 1,
          minColorTemp: 0,
          maxColorTemp: 0,
          maxDimSteps: 1,
          curve: RhythmConstantCurve(
            brightness: 1,
            colorTemp: 0,
            directColor: const RhythmDirectColor(
              rgb: RhythmRgbColor(r: 255, g: 53, b: 38),
              xy: RhythmXyColor(x: 0.604, y: 0.337),
            ),
          ),
        ),
      );

      final captured = verify(
        () => dio.post(
          'api/curve',
          queryParameters: captureAny(named: 'queryParameters'),
          data: captureAny(named: 'data'),
        ),
      ).captured;
      final body = captured[1] as Map<String, dynamic>;
      final curve = body['curve'] as Map<String, dynamic>;

      expect(curve['type'], 'constant');
      expect(curve['brightness'], 1);
      expect(curve['color_temp'], 0);
      expect(curve['direct_color'], isNotNull);
      expect(curve.containsKey('width_left_bri'), isFalse);
    });

    test('throws RhythmApiException on DioException', () async {
      when(() => dio.get('api/state')).thenThrow(
        DioException(
          requestOptions: RequestOptions(path: 'api/state'),
          response: Response(
            requestOptions: RequestOptions(path: 'api/state'),
            statusCode: 503,
          ),
        ),
      );

      expect(
        () => api.getCurveData(),
        throwsA(
          isA<RhythmApiException>()
              .having((e) => e.statusCode, 'statusCode', 503),
        ),
      );
    });
  });

  group('getCurveSolar', () {
    test('requests solar data for the requested date', () async {
      when(
        () => dio.get(
          'api/curve/solar',
          queryParameters: any(named: 'queryParameters'),
        ),
      ).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/curve/solar'),
          statusCode: 200,
          data: {
            'sunrise': 6.25,
            'sunrise_local_time': '06:15:00',
            'sunset': 19.75,
            'sunset_local_time': '19:45:00',
            'solar_noon': 13.0,
            'solar_noon_local_time': '13:00:00',
            'solar_midnight': 1.0,
            'solar_midnight_local_time': '01:00:00',
            'day_length': 13.5,
            'twilight': {
              'dawn': {'civil': 5.75},
              'dusk': {'civil': 20.25},
            },
          },
        ),
      );

      final solar = await api.getCurveSolar(date: DateTime.utc(2026, 6, 14));

      expect(solar.sunrise, 6.25);
      expect(solar.solarNoon, 13.0);
      expect(solar.dawn!.civil, 5.75);
      expect(solar.dusk!.civil, 20.25);
      final query = verify(
        () => dio.get(
          'api/curve/solar',
          queryParameters: captureAny(named: 'queryParameters'),
        ),
      ).captured.single as Map<String, dynamic>;
      expect(query['date'], '2026-06-14');
    });
  });

  group('getStepSequences', () {
    final previewJson = {
      'config': {
        'id': 'rhythm',
        'name': 'Rhythm Profile',
        'curve': {'type': 'super-gaussian'},
        'min_brightness': 2,
        'max_brightness': 100,
        'min_color_temp': 1800,
        'max_color_temp': 5500,
        'max_dim_steps': 12,
      },
      'solar': {
        'sunrise': 6.5,
        'sunset': 19.5,
        'solar_noon': 13.0,
        'solar_midnight': 1.0,
        'day_length': 13.0,
      },
      'curve': {
        'hours': [10.0, 10.5],
        'brightness': [80, 90],
        'kelvin': [4500, 5000],
      },
      'steps': {
        'start_hour': 10.0,
        'step_up': [
          {
            'hour': 10.0,
            'brightness': 80,
            'kelvin': 4500,
            'rgb': {'r': 255, 'g': 220, 'b': 180},
          },
          {
            'hour': 10.5,
            'brightness': 90,
            'kelvin': 5000,
            'rgb': {'r': 255, 'g': 230, 'b': 200},
          },
        ],
        'step_down': [
          {
            'hour': 10.0,
            'brightness': 80,
            'kelvin': 4500,
            'rgb': {'r': 255, 'g': 220, 'b': 180},
          },
          {
            'hour': 9.5,
            'brightness': 70,
            'kelvin': 4000,
            'rgb': {'r': 255, 'g': 210, 'b': 170},
          },
        ],
      },
    };

    test('uses the preview route with start_hour and max_steps', () async {
      stubActiveRhythmProfile();
      when(
        () => dio.get(
          'api/curve',
          queryParameters: any(named: 'queryParameters'),
        ),
      ).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/curve'),
          statusCode: 200,
          data: previewJson,
        ),
      );

      final result = await api.getStepSequences(hour: 10.0, maxSteps: 6);

      expect(result.stepUp, hasLength(2));
      expect(result.stepDown, hasLength(2));
      expect(result.stepUp.first.hour, 10.0);
      expect(result.stepUp.first.brightness, 80);
      expect(result.stepUp.first.kelvin, 4500);
      expect(result.stepUp.first.rgb, [255, 220, 180]);

      final query = verify(
        () => dio.get(
          'api/curve',
          queryParameters: captureAny(named: 'queryParameters'),
        ),
      ).captured.single as Map<String, dynamic>;
      expect(query['id'], 'rhythm');
      expect(query['start_hour'], 10.0);
      expect(query['max_steps'], 6);
    });

    test('throws RhythmApiException on DioException', () async {
      when(() => dio.get('api/state')).thenThrow(
        DioException(
          requestOptions: RequestOptions(path: 'api/state'),
          response: Response(
            requestOptions: RequestOptions(path: 'api/state'),
            statusCode: 422,
          ),
        ),
      );

      expect(
        () => api.getStepSequences(hour: 10.0, maxSteps: 6),
        throwsA(
          isA<RhythmApiException>()
              .having((e) => e.statusCode, 'statusCode', 422),
        ),
      );
    });
  });

  group('getTime', () {
    test('parses the current active-profile sample from /api/curve/now',
        () async {
      when(() => dio.get('api/state')).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/state'),
          statusCode: 200,
          data: {
            'settings': {
              'power_save': false,
            },
            'mode': {
              'active': 'sleep',
              'configs': [
                {
                  'mode': 'day',
                  'active_profile_id': 'rhythm',
                  'idle_profile_id': 'day_idle',
                },
                {
                  'mode': 'sleep',
                  'active_profile_id': 'sleep',
                  'idle_profile_id': 'sleep_idle',
                },
              ],
            },
            'profiles': [
              {
                'id': 'sleep',
                'name': 'Sleep Profile',
                'curve': {'type': 'super-gaussian'},
              },
            ],
            'active_profile': {
              'id': 'sleep',
              'name': 'Sleep Profile',
              'curve': {'type': 'super-gaussian'},
            },
          },
        ),
      );
      when(
        () => dio.get(
          'api/curve/now',
          queryParameters: any(named: 'queryParameters'),
        ),
      ).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/curve/now'),
          statusCode: 200,
          data: {
            'hour': 14.5,
            'brightness': 85,
            'kelvin': 4800,
            'sun_position': 0.65,
          },
        ),
      );

      final info = await api.getTime();

      expect(info.currentTime, '14:30');
      expect(info.currentHour, 14.5);
      expect(info.timezone, isNull);
      expect(info.brightness, 85);
      expect(info.kelvin, 4800);
      expect(info.solarPosition, 0.65);

      final query = verify(
        () => dio.get(
          'api/curve/now',
          queryParameters: captureAny(named: 'queryParameters'),
        ),
      ).captured.single as Map<String, dynamic>;
      expect(query['id'], 'sleep');
    });

    test('throws RhythmApiException on DioException', () async {
      when(() => dio.get('api/state')).thenThrow(
        DioException(
          requestOptions: RequestOptions(path: 'api/state'),
          response: Response(
            requestOptions: RequestOptions(path: 'api/state'),
            statusCode: 404,
          ),
        ),
      );

      expect(
        () => api.getTime(),
        throwsA(
          isA<RhythmApiException>()
              .having((e) => e.statusCode, 'statusCode', 404),
        ),
      );
    });
  });

  group('getState', () {
    test('parses the split bootstrap payload from /api/state', () async {
      when(() => dio.get('api/state')).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/state'),
          statusCode: 200,
          data: {
            'version': '1.2.3',
            'platform': 'linux',
            'context': 'embedded',
            'rooms': const <Map<String, dynamic>>[],
            'settings': {
              'power_save': false,
            },
            'mode': {
              'active': 'day',
              'configs': [
                {
                  'mode': 'day',
                  'active_profile_id': 'rhythm',
                  'idle_profile_id': 'day_idle',
                },
              ],
            },
            'profiles': [
              {
                'id': 'rhythm',
                'name': 'Rhythm Profile',
                'curve': {'type': 'super-gaussian'},
              },
            ],
            'active_profile': {
              'id': 'rhythm',
              'name': 'Rhythm Profile',
              'curve': {'type': 'super-gaussian'},
            },
            'location': <String, dynamic>{},
          },
        ),
      );

      final hello = await api.getState();

      expect(hello.settings, isNotNull);
      expect(hello.settings!.powerSave, isFalse);
      expect(hello.mode, isNotNull);
      expect(hello.mode!.active, RhythmMode.day);
      expect(hello.mode!.activeConfig?.activeProfileId, 'rhythm');
      expect(hello.profiles, hasLength(1));
      expect(hello.profiles.first.id, 'rhythm');
      verify(() => dio.get('api/state')).called(1);
    });

    test('handles hubs-only state payloads from /api/state', () async {
      when(() => dio.get('api/state')).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/state'),
          statusCode: 200,
          data: {
            'rooms': const <Map<String, dynamic>>[],
            'hubs': [
              {
                'type': 'homeassistant',
                'address': 'http://ha.local',
                'connected': true
              },
            ],
            'location': {
              'current_local_time': '2026-04-06T17:51:13-04:00',
            },
          },
        ),
      );

      final hello = await api.getState();

      expect(hello.hubs, hasLength(1));
      expect(hello.hub['type'], 'homeassistant');
      expect(
        hello.location['current_local_time'],
        '2026-04-06T17:51:13-04:00',
      );
    });

    test('parses Matter onboarding methods from /api/state capabilities',
        () async {
      when(() => dio.get('api/state')).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/state'),
          statusCode: 200,
          data: {
            'rooms': const <Map<String, dynamic>>[],
            'location': <String, dynamic>{},
            'capabilities': {
              'hubs': [
                {
                  'type': 'matter',
                  'configurable': true,
                  'device_onboarding_methods': [
                    'matter_on_network_setup_code',
                    'matter_ble_wifi_commissioning',
                  ],
                  'supports_unpairing': true,
                  'supports_roomless_devices': true,
                },
              ],
            },
          },
        ),
      );

      final hello = await api.getState();
      final matter = hello.capabilities?.hub('matter');

      expect(matter, isNotNull);
      expect(matter!.configurable, isTrue);
      expect(
        matter.deviceOnboardingMethods,
        containsAll([
          'matter_on_network_setup_code',
          'matter_ble_wifi_commissioning',
        ]),
      );
      expect(matter.supportsUnpairing, isTrue);
      expect(matter.supportsRoomlessDevices, isTrue);
      expect(matter.addDevice.onNetworkSetupCode, isTrue);
      expect(matter.addDevice.bleWifiCommissioning, isTrue);
      expect(matter.canAddDevice, isTrue);
    });

    test('parses review metadata from /api/state', () async {
      when(() => dio.get('api/state')).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/state'),
          statusCode: 200,
          data: {
            'rooms': const <Map<String, dynamic>>[],
            'location': <String, dynamic>{},
            'review': {
              'pending': {
                'devices': 2,
                'rooms': 1,
                'unassigned': 1,
                'hub_configured': 1,
                'total': 3,
              },
              'disconnected_hubs': [
                {
                  'type': 'hue',
                  'address': 'hue.local',
                },
              ],
              'preferred_endpoints': [
                {
                  'canonical_id': 'device-1',
                  'name': 'Kitchen Lamp',
                  'type': 'matter',
                  'hub_address': 'matter.local',
                  'native_id': 'node-44',
                  'endpoint_count': 2,
                },
              ],
              'hub_configured_conflicts': [
                {
                  'id': 'conflict-1',
                  'kind': 'hub_configured',
                  'status': 'pending',
                  'type': 'hue',
                  'hub_address': 'hue.local',
                  'native_id': 'light-1',
                  'name': 'Kitchen Lamp',
                  'created_at': 1710000000,
                  'summary':
                      'Native hub automation is still configured for this device',
                  'guidance': 'Remove it in Hue, then recheck.',
                },
              ],
              'triage_entries': [
                {
                  'id': 'history-1',
                  'kind': 'device_merge',
                  'status': 'new_device',
                  'type': 'matter',
                  'hub_address': 'matter.local',
                  'native_id': 'node-77',
                  'name': 'Desk Lamp',
                  'created_at': 1710000000,
                  'resolved_at': 1710000300,
                  'resolved_by': 'api',
                  'summary': 'Kept as a separate device',
                },
              ],
            },
          },
        ),
      );

      final hello = await api.getState();

      expect(hello.review.pending.total, 3);
      expect(hello.review.pending.unassigned, 1);
      expect(hello.review.disconnectedHubs.single.label, 'Hue - hue.local');
      expect(hello.review.preferredEndpoints.single.endpointCount, 2);
      expect(hello.review.hubConfiguredConflicts.single.guidance,
          'Remove it in Hue, then recheck.');
      expect(hello.review.triageEntries.single.isKeepSeparate, isTrue);
      expect(hello.review.resolvedEntries, hasLength(1));
    });

    test('keeps legacy add_device payloads working during rollout', () async {
      when(() => dio.get('api/state')).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/state'),
          statusCode: 200,
          data: {
            'rooms': const <Map<String, dynamic>>[],
            'location': <String, dynamic>{},
            'capabilities': {
              'hubs': [
                {
                  'type': 'matter',
                  'configurable': true,
                  'supports_unpairing': true,
                  'supports_roomless_devices': true,
                  'add_device': {
                    'on_network_setup_code': true,
                    'ble_wifi_commissioning': false,
                  },
                },
              ],
            },
          },
        ),
      );

      final hello = await api.getState();
      final matter = hello.capabilities?.hub('matter');

      expect(matter, isNotNull);
      expect(
        matter!.deviceOnboardingMethods,
        contains('matter_on_network_setup_code'),
      );
      expect(
          matter.deviceOnboardingMethods,
          isNot(contains(
            'matter_ble_wifi_commissioning',
          )));
      expect(matter.addDevice.onNetworkSetupCode, isTrue);
      expect(matter.addDevice.bleWifiCommissioning, isFalse);
      expect(matter.canAddDevice, isTrue);
    });
  });

  group('healthCheck', () {
    test('returns true when status is healthy', () async {
      when(() => dio.get('health')).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'health'),
          statusCode: 200,
          data: {'status': 'healthy'},
        ),
      );

      expect(await api.healthCheck(), isTrue);
      verify(() => dio.get('health')).called(1);
    });

    test('returns false when status is not healthy', () async {
      when(() => dio.get('health')).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'health'),
          statusCode: 200,
          data: {'status': 'degraded'},
        ),
      );

      expect(await api.healthCheck(), isFalse);
    });

    test('returns false on DioException', () async {
      when(() => dio.get('health')).thenThrow(
        DioException(
          requestOptions: RequestOptions(path: 'health'),
          type: DioExceptionType.connectionTimeout,
        ),
      );

      expect(await api.healthCheck(), isFalse);
    });
  });
}
