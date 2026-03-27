import 'package:dio/dio.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';
import 'package:mocktail/mocktail.dart';

import '../helpers/mock_dio.dart';

void main() {
  late MockDio dio;
  late RhythmConfigApi api;

  setUp(() {
    dio = MockDio();
    api = RhythmConfigApi(baseUrl: 'http://test/', dio: dio);
  });

  // ---------------------------------------------------------------------------
  // getConfigState
  // ---------------------------------------------------------------------------
  group('getConfigState', () {
    test('parses valid response', () async {
      when(() => dio.get(any())).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/config'),
            statusCode: 200,
            data: {
              'config': {
                'min_color_temp': 2000,
                'max_color_temp': 6000,
                'min_brightness': 5,
                'max_brightness': 100,
                'width_left_bri': 0.9,
                'width_right_bri': 0.8,
                'width_left_cct': 0.95,
                'width_right_cct': 1.1,
                'shape_p': 5.0,
                'max_dim_steps': 4,
              },
              'solar': {
                'sunrise': 6.0,
                'sunset': 20.0,
                'solar_noon': 12.0,
                'solar_midnight': 0.0,
                'day_length': 14.0,
              },
            },
          ));

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
      expect(state.solar.sunrise, 6.0);
      expect(state.solar.sunset, 20.0);
      expect(state.solar.solarNoon, 12.0);
      expect(state.solar.solarMidnight, 0.0);
      expect(state.solar.dayLength, 14.0);
      verify(() => dio.get('api/config')).called(1);
    });

    test('throws RhythmApiException on DioException', () async {
      when(() => dio.get(any())).thenThrow(DioException(
        requestOptions: RequestOptions(path: 'api/config'),
        response: Response(
          requestOptions: RequestOptions(path: 'api/config'),
          statusCode: 500,
        ),
      ));

      expect(
        () => api.getConfigState(),
        throwsA(isA<RhythmApiException>()
            .having((e) => e.statusCode, 'statusCode', 500)),
      );
    });

    test('throws RhythmApiException with null statusCode on network error',
        () async {
      when(() => dio.get(any())).thenThrow(DioException(
        requestOptions: RequestOptions(path: 'api/config'),
        type: DioExceptionType.connectionTimeout,
      ));

      expect(
        () => api.getConfigState(),
        throwsA(isA<RhythmApiException>()
            .having((e) => e.statusCode, 'statusCode', isNull)),
      );
    });
  });

  // ---------------------------------------------------------------------------
  // saveConfig
  // ---------------------------------------------------------------------------
  group('saveConfig', () {
    test('posts config JSON', () async {
      when(() => dio.post(any(), data: any(named: 'data')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(path: 'api/config'),
                statusCode: 200,
              ));

      final config = RhythmRawConfig(
        minColorTemp: 2000,
        maxColorTemp: 6000,
        minBrightness: 5,
        maxBrightness: 100,
        widthLeftBri: 0.9,
        widthRightBri: 0.8,
        widthLeftCct: 0.95,
        widthRightCct: 1.1,
        shapeP: 5.0,
        maxDimSteps: 4,
      );

      await api.saveConfig(config);

      final captured =
          verify(() => dio.post('api/config', data: captureAny(named: 'data')))
              .captured
              .single as Map<String, dynamic>;
      expect(captured['min_color_temp'], 2000);
      expect(captured['max_color_temp'], 6000);
    });

    test('throws RhythmApiException on DioException', () async {
      when(() => dio.post(any(), data: any(named: 'data')))
          .thenThrow(DioException(
        requestOptions: RequestOptions(path: 'api/config'),
        response: Response(
          requestOptions: RequestOptions(path: 'api/config'),
          statusCode: 400,
        ),
      ));

      expect(
        () => api.saveConfig(RhythmRawConfig.defaults()),
        throwsA(isA<RhythmApiException>()
            .having((e) => e.statusCode, 'statusCode', 400)),
      );
    });
  });

  // ---------------------------------------------------------------------------
  // getCurveData
  // ---------------------------------------------------------------------------
  group('getCurveData', () {
    final curveJson = {
      'hours': [0.0, 6.0, 12.0, 18.0, 24.0],
      'bris': [2, 50, 100, 50, 2],
      'ccts': [1800, 3500, 5500, 3500, 1800],
      'solar': {
        'sunrise': 6.5,
        'sunset': 19.5,
        'solarNoon': 13.0,
        'solarMidnight': 1.0,
        'dayLength': 13.0,
      },
    };

    test('sends no query params when none specified', () async {
      when(() => dio.get(any(), queryParameters: any(named: 'queryParameters')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(path: 'api/curve'),
                statusCode: 200,
                data: curveJson,
              ));

      final data = await api.getCurveData();

      expect(data.hours.length, 5);
      expect(data.brightness, [2, 50, 100, 50, 2]);
      expect(data.kelvin, [1800, 3500, 5500, 3500, 1800]);
      expect(data.solar.solarNoon, 13.0);

      final captured = verify(() => dio.get('api/curve',
              queryParameters:
                  captureAny(named: 'queryParameters'))).captured.single
          as Map<String, dynamic>;
      expect(captured, isEmpty);
    });

    test('includes month and overrides in query params', () async {
      when(() => dio.get(any(), queryParameters: any(named: 'queryParameters')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(path: 'api/curve'),
                statusCode: 200,
                data: curveJson,
              ));

      final overrides = RhythmCurveConfig(
        minBrightness: 10,
        maxBrightness: 90,
      );

      await api.getCurveData(month: 6, overrides: overrides);

      final captured = verify(() => dio.get('api/curve',
              queryParameters:
                  captureAny(named: 'queryParameters'))).captured.single
          as Map<String, dynamic>;
      expect(captured['month'], 6);
      expect(captured['min_brightness'], 10);
      expect(captured['max_brightness'], 90);
    });

    test('throws RhythmApiException on DioException', () async {
      when(() => dio.get(any(), queryParameters: any(named: 'queryParameters')))
          .thenThrow(DioException(
        requestOptions: RequestOptions(path: 'api/curve'),
        response: Response(
          requestOptions: RequestOptions(path: 'api/curve'),
          statusCode: 503,
        ),
      ));

      expect(
        () => api.getCurveData(),
        throwsA(isA<RhythmApiException>()
            .having((e) => e.statusCode, 'statusCode', 503)),
      );
    });
  });

  // ---------------------------------------------------------------------------
  // getStepSequences
  // ---------------------------------------------------------------------------
  group('getStepSequences', () {
    final stepsJson = {
      'step_up': {
        'steps': [
          {'hour': 10.0, 'brightness': 80, 'kelvin': 4500, 'rgb': [255, 220, 180]},
          {'hour': 10.5, 'brightness': 90, 'kelvin': 5000, 'rgb': [255, 230, 200]},
        ],
      },
      'step_down': {
        'steps': [
          {'hour': 10.0, 'brightness': 80, 'kelvin': 4500, 'rgb': [255, 220, 180]},
          {'hour': 9.5, 'brightness': 70, 'kelvin': 4000, 'rgb': [255, 210, 170]},
        ],
      },
    };

    test('sends hour and maxSteps as query params', () async {
      when(() => dio.get(any(), queryParameters: any(named: 'queryParameters')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(path: 'api/steps'),
                statusCode: 200,
                data: stepsJson,
              ));

      final result =
          await api.getStepSequences(hour: 10.0, maxSteps: 6);

      expect(result.stepUp.length, 2);
      expect(result.stepDown.length, 2);
      expect(result.stepUp[0].hour, 10.0);
      expect(result.stepUp[0].brightness, 80);
      expect(result.stepUp[0].kelvin, 4500);
      expect(result.stepUp[0].rgb, [255, 220, 180]);

      final captured = verify(() => dio.get('api/steps',
              queryParameters:
                  captureAny(named: 'queryParameters'))).captured.single
          as Map<String, dynamic>;
      expect(captured['hour'], 10.0);
      expect(captured['max_steps'], 6);
    });

    test('includes overrides in query params', () async {
      when(() => dio.get(any(), queryParameters: any(named: 'queryParameters')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(path: 'api/steps'),
                statusCode: 200,
                data: stepsJson,
              ));

      final overrides = RhythmCurveConfig(shapeP: 8.0);

      await api.getStepSequences(
          hour: 12.0, maxSteps: 4, overrides: overrides);

      final captured = verify(() => dio.get('api/steps',
              queryParameters:
                  captureAny(named: 'queryParameters'))).captured.single
          as Map<String, dynamic>;
      expect(captured['hour'], 12.0);
      expect(captured['max_steps'], 4);
      expect(captured['shape_p'], 8.0);
    });

    test('throws RhythmApiException on DioException', () async {
      when(() => dio.get(any(), queryParameters: any(named: 'queryParameters')))
          .thenThrow(DioException(
        requestOptions: RequestOptions(path: 'api/steps'),
        response: Response(
          requestOptions: RequestOptions(path: 'api/steps'),
          statusCode: 422,
        ),
      ));

      expect(
        () => api.getStepSequences(hour: 10.0, maxSteps: 6),
        throwsA(isA<RhythmApiException>()
            .having((e) => e.statusCode, 'statusCode', 422)),
      );
    });
  });

  // ---------------------------------------------------------------------------
  // getTime
  // ---------------------------------------------------------------------------
  group('getTime', () {
    test('parses valid response', () async {
      when(() => dio.get(any())).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/time'),
            statusCode: 200,
            data: {
              'current_time': '2026-03-26T14:30:00Z',
              'current_hour': 14.5,
              'timezone': 'America/Chicago',
              'lighting': {
                'brightness': 85,
                'kelvin': 4800,
                'solar_position': 0.65,
              },
            },
          ));

      final info = await api.getTime();

      expect(info.currentTime, '2026-03-26T14:30:00Z');
      expect(info.currentHour, 14.5);
      expect(info.timezone, 'America/Chicago');
      expect(info.brightness, 85);
      expect(info.kelvin, 4800);
      expect(info.solarPosition, 0.65);
      verify(() => dio.get('api/time')).called(1);
    });

    test('throws RhythmApiException on DioException', () async {
      when(() => dio.get(any())).thenThrow(DioException(
        requestOptions: RequestOptions(path: 'api/time'),
        response: Response(
          requestOptions: RequestOptions(path: 'api/time'),
          statusCode: 404,
        ),
      ));

      expect(
        () => api.getTime(),
        throwsA(isA<RhythmApiException>()
            .having((e) => e.statusCode, 'statusCode', 404)),
      );
    });
  });

  // ---------------------------------------------------------------------------
  // healthCheck
  // ---------------------------------------------------------------------------
  group('healthCheck', () {
    test('returns true when status is healthy', () async {
      when(() => dio.get(any())).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'health'),
            statusCode: 200,
            data: {'status': 'healthy'},
          ));

      expect(await api.healthCheck(), isTrue);
      verify(() => dio.get('health')).called(1);
    });

    test('returns false when status is not healthy', () async {
      when(() => dio.get(any())).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'health'),
            statusCode: 200,
            data: {'status': 'degraded'},
          ));

      expect(await api.healthCheck(), isFalse);
    });

    test('returns false on DioException (no throw)', () async {
      when(() => dio.get(any())).thenThrow(DioException(
        requestOptions: RequestOptions(path: 'health'),
        type: DioExceptionType.connectionTimeout,
      ));

      expect(await api.healthCheck(), isFalse);
    });

    test('returns false on arbitrary exception (catch-all)', () async {
      when(() => dio.get(any())).thenThrow(Exception('network down'));

      expect(await api.healthCheck(), isFalse);
    });
  });

  // ---------------------------------------------------------------------------
  // Error wrapping preserves statusCode
  // ---------------------------------------------------------------------------
  group('error wrapping', () {
    test('preserves statusCode from DioException across methods', () async {
      for (final code in [400, 401, 403, 404, 500, 502, 503]) {
        when(() => dio.get(any(),
            queryParameters: any(named: 'queryParameters'))).thenThrow(
          DioException(
            requestOptions: RequestOptions(path: ''),
            response: Response(
              requestOptions: RequestOptions(path: ''),
              statusCode: code,
            ),
          ),
        );
        when(() => dio.get(any())).thenThrow(
          DioException(
            requestOptions: RequestOptions(path: ''),
            response: Response(
              requestOptions: RequestOptions(path: ''),
              statusCode: code,
            ),
          ),
        );

        await expectLater(
          () => api.getConfigState(),
          throwsA(isA<RhythmApiException>()
              .having((e) => e.statusCode, 'statusCode', code)),
        );
        await expectLater(
          () => api.getTime(),
          throwsA(isA<RhythmApiException>()
              .having((e) => e.statusCode, 'statusCode', code)),
        );
      }
    });
  });
}
