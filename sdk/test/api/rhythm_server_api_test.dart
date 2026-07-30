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

      verify(() => dio.put('api/nodes/action', data: {
            'node_id': 'r1',
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

    test('sends correlation wrapper and parses rooms response', () async {
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
      ], correlationId: 'global-room-123');

      expect(result, hasLength(2));
      expect(result[0].roomId, 'r1');
      expect(result[1].roomId, 'r2');
      expect(cacheUpdates, hasLength(1));
      expect(cacheUpdates[0], hasLength(2));
      verify(() => dio.put(
            'api/nodes/action',
            data: {
              'nodes': [
                {'node_id': 'r1', 'action': 'enable'},
                {'node_id': 'r2', 'action': 'disable'},
              ],
              'correlation_id': 'global-room-123',
            },
            options: any(named: 'options'),
          )).called(1);
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
    test('dispatches reset actions through the node batch endpoint', () async {
      when(() => dio.put(any(),
              data: any(named: 'data'), options: any(named: 'options')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(path: 'api/nodes/action'),
                statusCode: 200,
                data: {
                  'nodes': [
                    {
                      'node_id': 'room-1',
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

      final result = await api.fixMyLights(nodeIds: ['room-1']);
      expect(result, hasLength(1));
      expect(result[0].nodeId, 'room-1');
      expect(result[0].brightness, 75);
      expect(cacheUpdates, hasLength(1));

      verify(() => dio.put(
            'api/nodes/action',
            data: [
              {
                'node_id': 'room-1',
                'action': 'reset',
              },
            ],
            options: any(named: 'options'),
          )).called(1);
    });

    test('returns empty list when no node ids are provided', () async {
      final result = await api.fixMyLights(nodeIds: const []);

      expect(result, isEmpty);
      verifyNever(() => dio.put(
            any(),
            data: any(named: 'data'),
            options: any(named: 'options'),
          ));
    });

    test('returns empty list on error', () async {
      when(() => dio.put(any(),
          data: any(named: 'data'),
          options: any(named: 'options'))).thenThrow(DioException(
        requestOptions: RequestOptions(path: 'api/nodes/action'),
      ));

      final result = await api.fixMyLights(nodeIds: ['room-1']);
      expect(result, isEmpty);
    });
  });

  group('node offset batch dispatch metadata', () {
    test('roomOffsetBatchResult parses queued response metadata', () async {
      when(() => dio.put(any(),
              data: any(named: 'data'), options: any(named: 'options')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(path: 'api/nodes/offset'),
                statusCode: 200,
                data: {
                  'nodes': [
                    {
                      'node_id': 'room-1',
                      'rhythm_enabled': true,
                      'time_offset': 30.0,
                      'brightness_offset': 0.0,
                      'state': 'active',
                    },
                    {
                      'node_id': 'room-2',
                      'rhythm_enabled': true,
                      'time_offset': 30.0,
                      'brightness_offset': 0.0,
                      'state': 'active',
                    },
                  ],
                  'queued': true,
                  'dispatch_count': 2,
                  'dispatch_spacing_ms': 500,
                  'estimated_dispatch_ms': 500,
                },
              ));

      final result = await api.roomOffsetBatchResult([
        (roomId: 'room-1', timeOffset: 30.0),
        (roomId: 'room-2', timeOffset: 30.0),
      ]);

      expect(result.states, hasLength(2));
      expect(result.queued, isTrue);
      expect(result.dispatchCount, 2);
      expect(result.dispatchSpacingMs, 500);
      expect(
          result.estimatedDispatchDuration, const Duration(milliseconds: 500));

      verify(() => dio.put(
            'api/nodes/offset',
            data: {
              'time_offset': 30.0,
              'nodes': ['room-1', 'room-2'],
            },
            options: any(named: 'options'),
          )).called(1);
    });

    test('nodeOffsetBatchResult can send custom dispatch spacing', () async {
      when(() => dio.put(any(),
              data: any(named: 'data'), options: any(named: 'options')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(path: 'api/nodes/offset'),
                statusCode: 200,
                data: {'nodes': const []},
              ));

      await api.nodeOffsetBatchResult(
        [
          (nodeId: 'room-1', timeOffset: 0.0),
          (nodeId: 'room-2', timeOffset: 0.0),
        ],
        dispatchSpacingMs: 250,
      );

      verify(() => dio.put(
            'api/nodes/offset',
            data: {
              'time_offset': 0.0,
              'nodes': ['room-1', 'room-2'],
              'dispatch_spacing_ms': 250,
            },
            options: any(named: 'options'),
          )).called(1);
    });

    test('nodeOffsetPreviewResult sends periodic tick scope by default',
        () async {
      when(() => dio.put(any(),
              data: any(named: 'data'), options: any(named: 'options')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(path: 'api/nodes/offset'),
                statusCode: 200,
                data: {'nodes': const []},
              ));

      await api.nodeOffsetPreviewResult(timeOffset: -40.0);

      verify(() => dio.put(
            'api/nodes/offset',
            data: {'time_offset': -40.0},
            options: any(named: 'options'),
          )).called(1);
    });
  });

  group('node preference writes', () {
    test('nodePreferencesSet sends profile_settings on the node endpoint',
        () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            queryParameters: any(named: 'queryParameters'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/nodes/preferences'),
            statusCode: 204,
          ));

      await api.nodePreferencesSet(
        nodeId: 'node-1',
        rhythmEnabled: true,
        profileSettings: {
          'motion_timeout_secs': 60,
          'motion_activation_enabled': false,
        },
      );

      verify(() => dio.put(
            'api/nodes/preferences',
            data: {
              'node_id': 'node-1',
              'rhythm_enabled': true,
              'profile_settings': {
                'motion_timeout_secs': {
                  'mode': 'fixed',
                  'value': 60,
                },
                'motion_activation_enabled': false,
              },
            },
            queryParameters: null,
          )).called(1);
    });

    test('nodeMotionActivationSet returns authoritative state and correlation',
        () async {
      when(() => dio.put(any(), data: any(named: 'data'))).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/nodes/motion-activation'),
          statusCode: 200,
          data: {
            'nodes': [
              {
                'node_id': 'node-1',
                'rhythm_enabled': true,
                'time_offset': 0.0,
                'brightness_offset': 0.0,
                'state': 'active',
                'profile_settings': {'motion_activation_enabled': false},
              },
            ],
          },
        ),
      );

      final state = await api.nodeMotionActivationSet(
        nodeId: 'node-1',
        enabled: false,
        requestId: 'motion-request-123',
      );

      expect(state, isNotNull);
      expect(state!.profileSettings?.motionActivationEnabled, isFalse);
      expect(cacheUpdates, hasLength(1));
      verify(() => dio.put(
            'api/nodes/motion-activation',
            data: {
              'node_id': 'node-1',
              'enabled': false,
              'request_id': 'motion-request-123',
            },
          )).called(1);
    });

    test('nodeMotionActivationSet returns null on rejected write', () async {
      when(() => dio.put(any(), data: any(named: 'data'))).thenThrow(
        DioException(
          requestOptions: RequestOptions(path: 'api/nodes/motion-activation'),
        ),
      );

      final state = await api.nodeMotionActivationSet(
        nodeId: 'node-1',
        enabled: false,
        requestId: 'motion-request-fail',
      );

      expect(state, isNull);
      expect(cacheUpdates, isEmpty);
    });

    test('nodeProfileOverridesSet sends profile override patch endpoint',
        () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            queryParameters: any(named: 'queryParameters'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/nodes/profile-overrides'),
            statusCode: 204,
          ));

      await api.nodeProfileOverridesSet(
        nodeId: 'node-1',
        profileOverrides: {
          'rhythm': {
            'min_brightness': 8,
            'max_brightness': 72,
            'rhythm_interval_secs': 90,
            'motion_timeout_secs': 600,
          },
          'sleep': null,
        },
        replace: true,
        correlationId: 'room-light-settings-123',
      );

      verify(() => dio.put(
            'api/nodes/profile-overrides',
            data: {
              'node_id': 'node-1',
              'profile_overrides': {
                'rhythm': {
                  'min_brightness': 8,
                  'max_brightness': 72,
                  'rhythm_interval_secs': {
                    'mode': 'fixed',
                    'value': 90,
                  },
                  'motion_timeout_secs': {
                    'mode': 'fixed',
                    'value': 600,
                  },
                },
                'sleep': null,
              },
              'replace': true,
              'correlation_id': 'room-light-settings-123',
            },
            queryParameters: null,
          )).called(1);
    });

    test('nodeProfileOverridesSet reports a rejected request', () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            queryParameters: any(named: 'queryParameters'),
          )).thenThrow(
        DioException(
          requestOptions: RequestOptions(path: 'api/nodes/profile-overrides'),
        ),
      );

      final accepted = await api.nodeProfileOverridesSet(
        nodeId: 'node-1',
        profileOverrides: {
          'rhythm': {'min_brightness': 8},
        },
        replace: true,
      );

      expect(accepted, isFalse);
    });

    test('nodePreferencesSet sends mood state', () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            queryParameters: any(named: 'queryParameters'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/nodes/preferences'),
            statusCode: 204,
          ));

      await api.nodePreferencesSet(
        nodeId: 'node-1',
        rhythmEnabled: true,
        state: RoomModeState.mood,
      );

      verify(() => dio.put(
            'api/nodes/preferences',
            data: {
              'node_id': 'node-1',
              'rhythm_enabled': true,
              'state': 'mood',
            },
            queryParameters: null,
          )).called(1);
    });

    test('nodePreferencesSet sends standby for idle compatibility state',
        () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            queryParameters: any(named: 'queryParameters'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/nodes/preferences'),
            statusCode: 204,
          ));

      await api.nodePreferencesSet(
        nodeId: 'node-1',
        rhythmEnabled: true,
        standbyEnabled: true,
        state: RoomModeState.idle,
      );

      verify(() => dio.put(
            'api/nodes/preferences',
            data: {
              'node_id': 'node-1',
              'rhythm_enabled': true,
              'standby_enabled': true,
              'state': 'standby',
            },
            queryParameters: null,
          )).called(1);
    });

    test('nodePreferencesBatchSet normalizes room_profile to profile_settings',
        () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            options: any(named: 'options'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/nodes/preferences'),
            statusCode: 200,
            data: {'nodes': const []},
          ));

      await api.nodePreferencesBatchSet([
        {
          'room_id': 'room-1',
          'disabled': true,
          'room_profile': {
            'fade_ms': 1200,
            'motion_timeout_secs': 90,
            'rhythm_interval_secs': 60,
          },
        },
      ]);

      verify(() => dio.put(
            'api/nodes/preferences',
            data: [
              {
                'node_id': 'room-1',
                'disabled': true,
                'profile_settings': {
                  'fade_ms': {
                    'mode': 'fixed',
                    'value': 1200,
                  },
                  'motion_timeout_secs': {
                    'mode': 'fixed',
                    'value': 90,
                  },
                },
              },
            ],
            options: any(named: 'options'),
          )).called(1);
    });

    test('nodePreferencesBatchSet can send dispatch spacing metadata',
        () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            options: any(named: 'options'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/nodes/preferences'),
            statusCode: 200,
            data: {
              'nodes': const [],
              'queued': true,
              'dispatch_count': 2,
              'dispatch_spacing_ms': 250,
              'estimated_dispatch_ms': 250,
            },
          ));

      final result = await api.nodePreferencesBatchSetResult(
        [
          {'node_id': 'room-1', 'state': 'active'},
          {'node_id': 'room-2', 'state': 'hard_off'},
        ],
        dispatchSpacingMs: 250,
      );

      expect(result.queued, isTrue);
      expect(result.dispatchSpacingMs, 250);
      verify(() => dio.put(
            'api/nodes/preferences',
            data: {
              'nodes': [
                {'node_id': 'room-1', 'state': 'active'},
                {'node_id': 'room-2', 'state': 'hard_off'},
              ],
              'dispatch_spacing_ms': 250,
            },
            options: any(named: 'options'),
          )).called(1);
    });
  });

  group('motion timeout writes', () {
    test('motionTimeoutSet falls back to node preferences endpoint', () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            queryParameters: any(named: 'queryParameters'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/nodes/preferences'),
            statusCode: 204,
          ));

      await api.motionTimeoutSet(nodeId: 'node-1', timeoutSecs: null);

      verify(() => dio.put(
            'api/nodes/preferences',
            data: {
              'node_id': 'node-1',
              'profile_settings': {
                'motion_timeout_secs': null,
              },
            },
            queryParameters: null,
          )).called(1);
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
  // light breaker
  // ---------------------------------------------------------------------------
  group('light breaker', () {
    test('getLightBreaker parses enabled state', () async {
      when(() => dio.get(any())).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/light-breaker'),
            statusCode: 200,
            data: {'enabled': false},
          ));

      final lightBreaker = await api.getLightBreaker();

      expect(lightBreaker, isNotNull);
      expect(lightBreaker!.enabled, isFalse);
      verify(() => dio.get('api/light-breaker')).called(1);
    });

    test('setLightBreaker sends enabled to /api/light-breaker', () async {
      when(() => dio.put(any(), data: any(named: 'data')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(path: 'api/light-breaker'),
                statusCode: 200,
              ));

      final saved = await api.setLightBreaker(false);

      expect(saved, isTrue);
      final captured = verify(() => dio.put(
            'api/light-breaker',
            data: captureAny(named: 'data'),
          )).captured.single;
      expect(captured, {'enabled': false});
    });

    test('setLightBreaker returns false on DioException', () async {
      when(() => dio.put(any(), data: any(named: 'data')))
          .thenThrow(DioException(
        requestOptions: RequestOptions(path: 'api/light-breaker'),
      ));

      final saved = await api.setLightBreaker(false);

      expect(saved, isFalse);
    });
  });

  // ---------------------------------------------------------------------------
  // light runtime
  // ---------------------------------------------------------------------------
  group('light runtime', () {
    test('getLightRuntime parses selected runtime', () async {
      when(() => dio.get(any())).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/light-runtime'),
            statusCode: 200,
            data: {
              'runtime_id': 'rhythm-adaptive',
              'available_runtime_ids': ['rhythm-adaptive'],
            },
          ));

      final state = await api.getLightRuntime();

      expect(state, isNotNull);
      expect(state!.runtime, RhythmLightRuntime.rhythmAdaptive);
      verify(() => dio.get('api/light-runtime')).called(1);
    });

    test('setLightRuntime sends runtime_id to /api/light-runtime', () async {
      when(() => dio.put(any(), data: any(named: 'data')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(path: 'api/light-runtime'),
                statusCode: 200,
                data: {'runtime_id': 'rhythm-adaptive'},
              ));

      final state = await api.setLightRuntime(
        RhythmLightRuntime.rhythmAdaptive,
        transitionMs: 3000,
      );

      expect(state?.runtime, RhythmLightRuntime.rhythmAdaptive);
      final captured = verify(() => dio.put(
            'api/light-runtime',
            data: captureAny(named: 'data'),
          )).captured.single;
      expect(captured, {
        'runtime_id': 'rhythm-adaptive',
        'transition_ms': 3000,
      });
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

    test('ignores removed power_save when it is the only setting', () async {
      final saved = await api.settingsSet(powerSave: true);

      expect(saved, isTrue);
      verifyNever(() => dio.put(
            any(),
            data: any(named: 'data'),
            queryParameters: any(named: 'queryParameters'),
          ));
    });

    test('returns false on DioException', () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            queryParameters: any(named: 'queryParameters'),
          )).thenThrow(DioException(
        requestOptions: RequestOptions(path: 'api/settings'),
      ));

      final saved = await api.settingsSet(autoUpdate: true);

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
            'room_defaults': [],
          },
        ],
      });
    });
  });

  group('configSet', () {
    test('sends full profile config with timer-setting payloads', () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            queryParameters: any(named: 'queryParameters'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/config'),
            statusCode: 204,
          ));

      const saved = RhythmCurveConfig(
        id: 'day',
        name: 'Day',
        fadeSetting: RhythmTimerSetting.auto(),
        motionTimeoutSetting: RhythmTimerSetting.fixed(300),
        rhythmIntervalSetting: RhythmTimerSetting.scheduled([
          RhythmTimerBreakpoint(hour: 8.0, value: 60),
        ]),
      );

      final ok = await api.configSet(saved);

      expect(ok, isTrue);
      final captured = verify(() => dio.put(
            'api/config',
            data: captureAny(named: 'data'),
            queryParameters: captureAny(named: 'queryParameters'),
          )).captured;
      expect(captured[0], containsPair('id', 'day'));
      expect(captured[0], containsPair('fade_ms', {'mode': 'auto'}));
      expect(
          captured[0],
          containsPair('motion_timeout_secs', {
            'mode': 'fixed',
            'value': 300,
          }));
      expect(
          captured[0],
          containsPair('rhythm_interval_secs', {
            'mode': 'scheduled',
            'breakpoints': [
              {'hour': 8.0, 'value': 60},
            ],
          }));
      expect(captured[1], {'id': 'day'});
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

  group('topology', () {
    test('getTopologyNodes parses the bare array response', () async {
      when(() => dio.get(any())).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/topology/nodes'),
            statusCode: 200,
            data: [
              {
                'id': 'room-1',
                'name': 'Living Room',
                'kind': 'room',
              },
              {
                'id': 'bulb-1',
                'name': 'Lamp',
                'kind': 'light_device',
                'parent_id': 'room-1',
                'placement': 'standalone',
                'controls': [
                  {
                    'kind': 'motion',
                    'target_id': 'room-1',
                    'inherited': false,
                  },
                ],
                'manufacturer': 'Signify',
                'model': 'Hue Color',
              },
            ],
          ));

      final nodes = await api.getTopologyNodes();

      expect(nodes, hasLength(2));
      expect(nodes.first.kind, RhythmNodeKind.room);
      expect(nodes.last.kind, RhythmNodeKind.lightDevice);
      expect(nodes.last.parentId, 'room-1');
      expect(nodes.last.placement, RhythmNodePlacement.standalone);
      expect(nodes.last.controls, hasLength(1));
      expect(nodes.last.controls.first.targetId, 'room-1');
      verify(() => dio.get('api/topology/nodes')).called(1);
    });

    test('setTopologyNodeControlTarget sends target_id including null',
        () async {
      when(() => dio.put(any(), data: any(named: 'data')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(
                  path: 'api/topology/nodes/sensor-1/controls/motion',
                ),
                statusCode: 200,
              ));

      final saved = await api.setTopologyNodeControlTarget(
        nodeId: 'sensor-1',
        controlKind: 'motion',
        targetId: null,
      );

      expect(saved, isTrue);
      verify(() => dio.put(
            'api/topology/nodes/sensor-1/controls/motion',
            data: {'target_id': null},
          )).called(1);
    });

    test('setTopologyNodeControlTargets sends every target_id', () async {
      when(() => dio.put(any(), data: any(named: 'data')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(
                  path: 'api/topology/nodes/sensor-1/controls/motion',
                ),
                statusCode: 200,
              ));

      final saved = await api.setTopologyNodeControlTargets(
        nodeId: 'sensor-1',
        controlKind: 'motion',
        targetIds: ['room-1', 'room-2'],
      );

      expect(saved, isTrue);
      verify(() => dio.put(
            'api/topology/nodes/sensor-1/controls/motion',
            data: {
              'target_ids': ['room-1', 'room-2'],
            },
          )).called(1);
    });

    test('assignDeviceParent sends parent_id including null', () async {
      when(() => dio.put(any(), data: any(named: 'data')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(
                  path: 'api/devices/canonical/device-1/parent',
                ),
                statusCode: 204,
              ));

      final saved = await api.assignDeviceParent('device-1', null);

      expect(saved, isTrue);
      verify(() => dio.put(
            'api/devices/canonical/device-1/parent',
            data: {'parent_id': null},
          )).called(1);
    });
  });

  group('hub retry', () {
    test('posts the manual retry request body and returns true on 204',
        () async {
      when(() => dio.post(any(), data: any(named: 'data'))).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/hub/retry'),
          statusCode: 204,
        ),
      );

      final ok = await api.hubRetry(
        hubType: 'hue',
        address: '192.168.1.2',
      );

      expect(ok, isTrue);
      verify(() => dio.post('api/hub/retry', data: {
            'hub_type': 'hue',
            'address': '192.168.1.2',
          })).called(1);
    });

    test('returns false on DioException', () async {
      when(() => dio.post(any(), data: any(named: 'data'))).thenThrow(
        DioException(
          requestOptions: RequestOptions(path: 'api/hub/retry'),
          type: DioExceptionType.badResponse,
        ),
      );

      final ok = await api.hubRetry(
        hubType: 'hue',
        address: '192.168.1.2',
      );

      expect(ok, isFalse);
    });
  });

  group('input bindings', () {
    Map<String, dynamic> bindingJson({
      String id = 'day_sleep_toggle:button-1:on_press',
      String sourceNodeId = 'button-1',
    }) {
      return {
        'id': id,
        'preset': 'day_sleep_toggle',
        'source_node_id': sourceNodeId,
        'trigger': {
          'kind': 'button',
          'button_action': 'on_press',
        },
        'action': {
          'kind': 'mode_cycle',
          'modes': ['day', 'sleep'],
          'transition': {'kind': 'auto'},
        },
        'enabled': true,
      };
    }

    test('getInputBindings parses the bindings wrapper', () async {
      when(() => dio.get(any())).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/input-bindings'),
            statusCode: 200,
            data: {
              'bindings': [bindingJson()],
            },
          ));

      final bindings = await api.getInputBindings();

      expect(bindings, hasLength(1));
      expect(bindings.single.preset, RhythmInputBindingPreset.daySleepToggle);
      verify(() => dio.get('api/input-bindings')).called(1);
    });

    test('createDaySleepToggleInputBinding posts the preset request', () async {
      when(() => dio.post(any(), data: any(named: 'data')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(path: 'api/input-bindings'),
                statusCode: 200,
                data: {
                  'bindings': [bindingJson()],
                },
              ));

      final bindings = await api.createDaySleepToggleInputBinding(
        sourceNodeId: 'button-1',
      );

      expect(bindings.single.id, 'day_sleep_toggle:button-1:on_press');
      verify(() => dio.post('api/input-bindings', data: {
            'preset': 'day_sleep_toggle',
            'source_node_id': 'button-1',
            'button_action': 'on_press',
            'enabled': true,
          })).called(1);
    });

    test('setPresetInputBinding puts an explicit binding id', () async {
      when(() => dio.put(any(), data: any(named: 'data')))
          .thenAnswer((_) async => Response(
                requestOptions:
                    RequestOptions(path: 'api/input-bindings/bedroom'),
                statusCode: 200,
                data: {
                  'bindings': [bindingJson(id: 'bedroom')],
                },
              ));

      final bindings = await api.setPresetInputBinding(
        id: 'bedroom',
        preset: RhythmInputBindingPreset.daySleepToggle,
        sourceNodeId: 'button-1',
        buttonAction: RhythmButtonAction.offPress,
        enabled: false,
      );

      expect(bindings.single.id, 'bedroom');
      verify(() => dio.put('api/input-bindings/bedroom', data: {
            'preset': 'day_sleep_toggle',
            'source_node_id': 'button-1',
            'button_action': 'off_press',
            'enabled': false,
          })).called(1);
    });

    test('setInputBinding puts generic trigger and action JSON', () async {
      when(() => dio.put(any(), data: any(named: 'data')))
          .thenAnswer((_) async => Response(
                requestOptions:
                    RequestOptions(path: 'api/input-bindings/custom'),
                statusCode: 200,
                data: {
                  'bindings': [bindingJson(id: 'custom')],
                },
              ));

      const binding = RhythmInputBinding(
        id: 'custom',
        sourceNodeId: 'button-1',
        trigger: RhythmInputBindingTrigger.button(
          buttonAction: RhythmButtonAction.offPress,
        ),
        action: RhythmModeCycleAction(
          modes: [RhythmMode.day, RhythmMode.sleep],
          transition: RhythmModeTransitionSelection.none(),
        ),
      );

      await api.setInputBinding(binding);

      verify(() => dio.put('api/input-bindings/custom', data: {
            'id': 'custom',
            'source_node_id': 'button-1',
            'trigger': {
              'kind': 'button',
              'button_action': 'off_press',
            },
            'action': {
              'kind': 'mode_cycle',
              'modes': ['day', 'sleep'],
              'transition': {'kind': 'none'},
            },
            'enabled': true,
          })).called(1);
    });

    test('deleteInputBinding returns the updated binding list', () async {
      when(() => dio.delete(any())).thenAnswer((_) async => Response(
            requestOptions:
                RequestOptions(path: 'api/input-bindings/old-binding'),
            statusCode: 200,
            data: {'bindings': <dynamic>[]},
          ));

      final bindings = await api.deleteInputBinding('old-binding');

      expect(bindings, isEmpty);
      verify(() => dio.delete('api/input-bindings/old-binding')).called(1);
    });
  });

  group('scene endpoints', () {
    Map<String, dynamic> sceneJson({String id = 'icy-glow'}) => {
          'id': id,
          'name': 'Icy Glow',
          'description': null,
          'source': {'kind': 'user'},
          'light': {
            'default_transition_ms': 400,
            'default_output': {
              'power': 'on',
              'brightness': 72,
              'color': {'kind': 'kelvin', 'kelvin': 6500},
            },
            'entries': const [],
          },
          'extensions': const {},
        };

    test('getScenes parses the scenes wrapper', () async {
      when(() => dio.get(any())).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/scenes'),
            statusCode: 200,
            data: {
              'scenes': [sceneJson()],
            },
          ));

      final scenes = await api.getScenes();

      expect(scenes, hasLength(1));
      expect(scenes.single.id, 'icy-glow');
      expect(scenes.single.light.defaultOutput?.color?.kelvin, 6500);
      verify(() => dio.get('api/scenes')).called(1);
    });

    test('getScenes sends room target for native scene discovery', () async {
      when(() => dio.get(
            any(),
            queryParameters: any(named: 'queryParameters'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/scenes'),
            statusCode: 200,
            data: {
              'scenes': [sceneJson(id: 'hue-arctic')],
            },
          ));

      final scenes = await api.getScenes(targetId: 'room-1');

      expect(scenes.single.id, 'hue-arctic');
      verify(() => dio.get(
            'api/scenes',
            queryParameters: {'target_id': 'room-1'},
          )).called(1);
    });

    test('getSceneCatalog exposes partial native discovery failure', () async {
      when(() => dio.get(
            any(),
            queryParameters: any(named: 'queryParameters'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/scenes'),
            statusCode: 200,
            data: {
              'scenes': [sceneJson(id: 'stored-scene')],
              'native_discovery_failed': true,
            },
          ));

      final catalog = await api.getSceneCatalog(targetId: 'room-1');

      expect(catalog, isNotNull);
      expect(catalog!.scenes.single.id, 'stored-scene');
      expect(catalog.nativeDiscoveryFailed, isTrue);
    });

    test('getSceneCatalog defaults missing discovery metadata to complete',
        () async {
      when(() => dio.get(
            any(),
            queryParameters: any(named: 'queryParameters'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/scenes'),
            statusCode: 200,
            data: {
              'scenes': [sceneJson(id: 'stored-scene')],
            },
          ));

      final catalog = await api.getSceneCatalog(targetId: 'room-1');

      expect(catalog, isNotNull);
      expect(catalog!.nativeDiscoveryFailed, isFalse);
    });

    test('getSceneCatalog treats malformed or failed responses as failures',
        () async {
      when(() => dio.get(any())).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/scenes'),
            statusCode: 200,
            data: {'unexpected': true},
          ));

      expect(await api.getSceneCatalog(), isNull);

      when(() => dio.get(any())).thenThrow(DioException(
        requestOptions: RequestOptions(path: 'api/scenes'),
      ));

      expect(await api.getSceneCatalog(), isNull);
    });

    test('upsertScene posts the scene definition', () async {
      when(() => dio.post(any(), data: any(named: 'data')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(path: 'api/scenes'),
                statusCode: 200,
                data: sceneJson(),
              ));

      final scene = RhythmSceneDefinition.fromJson(sceneJson());
      final saved = await api.upsertScene(scene);

      expect(saved?.id, 'icy-glow');
      verify(() => dio.post('api/scenes', data: sceneJson())).called(1);
    });

    test('putScene uses path id while sending the body scene', () async {
      when(() => dio.put(any(), data: any(named: 'data')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(path: 'api/scenes/path-id'),
                statusCode: 200,
                data: sceneJson(id: 'path-id'),
              ));

      final scene = RhythmSceneDefinition.fromJson(sceneJson(id: 'body-id'));
      final saved = await api.putScene('path-id', scene);

      expect(saved?.id, 'path-id');
      verify(() => dio.put(
            'api/scenes/path-id',
            data: sceneJson(id: 'body-id'),
          )).called(1);
    });

    test('deleteScene returns the authoritative scenes list', () async {
      when(() => dio.delete(any())).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/scenes/icy-glow'),
            statusCode: 200,
            data: {
              'scenes': [sceneJson(id: 'other-scene')],
            },
          ));

      final scenes = await api.deleteScene('icy-glow');

      expect(scenes.single.id, 'other-scene');
      verify(() => dio.delete('api/scenes/icy-glow')).called(1);
    });

    test('applyScene posts target and parses affected nodes', () async {
      when(() => dio.post(any(), data: any(named: 'data')))
          .thenAnswer((_) async => Response(
                requestOptions:
                    RequestOptions(path: 'api/scenes/icy-glow/apply'),
                statusCode: 200,
                data: {
                  'scene_id': 'icy-glow',
                  'target_id': 'room1',
                  'affected_node_ids': ['light-node-1'],
                  'unresolved_node_ids': const [],
                },
              ));

      final result = await api.applyScene(
        sceneId: 'icy-glow',
        targetId: 'room1',
        transitionMs: 250,
        correlationId: 'mood-scene-123',
      );

      expect(result?.sceneId, 'icy-glow');
      expect(result?.targetId, 'room1');
      expect(result?.affectedNodeIds, ['light-node-1']);
      verify(() => dio.post(
            'api/scenes/icy-glow/apply',
            data: {
              'target_id': 'room1',
              'transition_ms': 250,
              'correlation_id': 'mood-scene-123',
            },
          )).called(1);
    });

    test('previewScene posts duration and returns preview id', () async {
      when(() => dio.post(any(), data: any(named: 'data')))
          .thenAnswer((_) async => Response(
                requestOptions:
                    RequestOptions(path: 'api/scenes/icy-glow/preview'),
                statusCode: 200,
                data: {
                  'scene_id': 'icy-glow',
                  'target_id': 'room1',
                  'affected_node_ids': ['light-node-1'],
                  'unresolved_node_ids': const [],
                  'preview_id': 'preview-2',
                },
              ));

      final result = await api.previewScene(
        sceneId: 'icy-glow',
        targetId: 'room1',
        durationMs: 30000,
      );

      expect(result?.previewId, 'preview-2');
      expect(result?.hasPreviewId, isTrue);
      verify(() => dio.post(
            'api/scenes/icy-glow/preview',
            data: {'target_id': 'room1', 'duration_ms': 30000},
          )).called(1);
    });

    test('previewDraftScene sends unsaved scene in the request body', () async {
      when(() => dio.post(any(), data: any(named: 'data')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(path: 'api/scenes/preview'),
                statusCode: 200,
                data: {
                  'scene_id': 'draft-scene',
                  'target_id': 'room1',
                  'affected_node_ids': const [],
                  'unresolved_node_ids': const [],
                  'preview_id': 'draft-preview',
                },
              ));

      final scene =
          RhythmSceneDefinition.fromJson(sceneJson(id: 'draft-scene'));
      final result = await api.previewDraftScene(
        scene: scene,
        targetId: 'room1',
        transitionMs: 250,
      );

      expect(result?.previewId, 'draft-preview');
      verify(() => dio.post(
            'api/scenes/preview',
            data: {
              'scene': sceneJson(id: 'draft-scene'),
              'target_id': 'room1',
              'transition_ms': 250,
            },
          )).called(1);
    });

    test('commit and cancel preview use preview token endpoints', () async {
      when(() => dio.post(any(), data: any(named: 'data')))
          .thenAnswer((invocation) async {
        final path = invocation.positionalArguments.single as String;
        return Response(
          requestOptions: RequestOptions(path: path),
          statusCode: 200,
          data: {
            'scene_id': 'icy-glow',
            'target_id': 'room1',
            'affected_node_ids': const [],
            'unresolved_node_ids': const [],
            if (path.endsWith('/commit')) 'preview_id': 'preview-1',
          },
        );
      });

      final committed = await api.commitScenePreview('preview-1');
      final canceled = await api.cancelScenePreview('preview-2');

      expect(committed?.sceneId, 'icy-glow');
      expect(canceled?.sceneId, 'icy-glow');
      verify(() => dio.post(
            'api/scene-previews/preview-1/commit',
            data: const <String, dynamic>{},
          )).called(1);
      verify(() => dio.post(
            'api/scene-previews/preview-2/cancel',
            data: const <String, dynamic>{},
          )).called(1);
    });

    test('nodeMoodSceneSet writes mood_scene_id and enters Mood', () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            queryParameters: any(named: 'queryParameters'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/nodes/preferences'),
            statusCode: 204,
          ));

      await api.nodeMoodSceneSet(nodeId: 'room1', sceneId: 'icy-glow');

      verify(() => dio.put(
            'api/nodes/preferences',
            data: {
              'node_id': 'room1',
              'state': 'mood',
              'profile_settings': {'mood_scene_id': 'icy-glow'},
            },
            queryParameters: null,
          )).called(1);
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

    test('nodeCurveBrightness posts a curve modifier payload', () async {
      when(() => dio.put(any(), data: any(named: 'data')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(path: 'api/nodes/curve'),
                statusCode: 200,
                data: {
                  'node_id': 'node-1',
                  'rhythm_enabled': true,
                  'time_offset': 0.0,
                  'brightness_offset': -10.0,
                  'state': 'active',
                  'lights_on': true,
                  'brightness': 45,
                  'kelvin': 3200,
                },
              ));

      final state = await api.nodeCurveBrightness(
        nodeId: 'node-1',
        brightness: 45,
      );

      expect(state, isNotNull);
      expect(state!.nodeId, 'node-1');
      expect(state.brightness, 45);
      verify(() => dio.put('api/nodes/curve', data: {
            'node_id': 'node-1',
            'brightness': 45,
          })).called(1);
      expect(cacheUpdates.single.single.nodeId, 'node-1');
    });

    test('nodeBrightness uses the authoritative brightness endpoint', () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            queryParameters: any(named: 'queryParameters'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/nodes/brightness'),
            statusCode: 200,
          ));

      await api.nodeBrightness(nodeId: 'node-1', brightness: 50);

      verify(() => dio.put(
            'api/nodes/brightness',
            data: {
              'node_id': 'node-1',
              'brightness': 50,
            },
            queryParameters: null,
          )).called(1);
    });

    test('nodeBrightnessResult parses returned state', () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            queryParameters: any(named: 'queryParameters'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/nodes/brightness'),
            statusCode: 200,
            data: {
              'nodes': [
                {
                  'node_id': 'node-1',
                  'rhythm_enabled': true,
                  'time_offset': 0.0,
                  'brightness_offset': 0.0,
                  'state': 'mood',
                  'brightness': 50,
                },
              ],
            },
          ));

      final state = await api.nodeBrightnessResult(
        nodeId: 'node-1',
        brightness: 50,
      );

      expect(state?.nodeId, 'node-1');
      expect(state?.state, RoomModeState.mood);
      expect(state?.brightness, 50);
      expect(cacheUpdates.single.single.nodeId, 'node-1');
    });

    test('nodeBrightnessBatch posts nodes to the brightness endpoint',
        () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            options: any(named: 'options'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/nodes/brightness'),
            statusCode: 200,
            data: {'nodes': <dynamic>[]},
          ));

      await api.nodeBrightnessBatchResult([
        (nodeId: 'node-1', brightness: 42),
        (nodeId: 'node-2', brightness: 64),
      ], dispatchSpacingMs: 20);

      verify(() => dio.put(
            'api/nodes/brightness',
            data: {
              'nodes': [
                {'node_id': 'node-1', 'brightness': 42},
                {'node_id': 'node-2', 'brightness': 64},
              ],
              'dispatch_spacing_ms': 20,
            },
            options: any(named: 'options'),
          )).called(1);
    });

    test('nodeCurveColorTemperature posts a curve modifier payload', () async {
      when(() => dio.put(any(), data: any(named: 'data')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(path: 'api/nodes/curve'),
                statusCode: 200,
                data: {
                  'nodes': [
                    {
                      'node_id': 'node-1',
                      'rhythm_enabled': true,
                      'time_offset': -180.0,
                      'brightness_offset': 8.0,
                      'state': 'active',
                      'lights_on': true,
                      'brightness': 55,
                      'kelvin': 3000,
                    },
                  ],
                },
              ));

      final state = await api.nodeCurveColorTemperature(
        nodeId: 'node-1',
        kelvin: 3000,
      );

      expect(state, isNotNull);
      expect(state!.kelvin, 3000);
      verify(() => dio.put('api/nodes/curve', data: {
            'node_id': 'node-1',
            'color_temperature': 3000,
            'preserve_brightness': true,
          })).called(1);
      expect(cacheUpdates.single.single.kelvin, 3000);
    });

    test('nodeCurveBrightnessBatch posts nodes to the curve endpoint',
        () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            options: any(named: 'options'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/nodes/curve'),
            statusCode: 200,
            data: {'nodes': <dynamic>[]},
          ));

      await api.nodeCurveBrightnessBatchResult([
        (nodeId: 'node-1', brightness: 42),
        (nodeId: 'node-2', brightness: 64),
      ], dispatchSpacingMs: 20, correlationId: 'global-room-brightness-123');

      verify(() => dio.put(
            'api/nodes/curve',
            data: {
              'nodes': [
                {'node_id': 'node-1', 'brightness': 42},
                {'node_id': 'node-2', 'brightness': 64},
              ],
              'dispatch_spacing_ms': 20,
              'correlation_id': 'global-room-brightness-123',
            },
            options: any(named: 'options'),
          )).called(1);
    });

    test('nodeColor sends normalized rgb payload with mood scope', () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            queryParameters: any(named: 'queryParameters'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/nodes/color'),
            statusCode: 200,
          ));

      await api.nodeColor(
        nodeId: 'node-1',
        r: 255,
        g: 128,
        b: 32,
        brightness: 12,
        transitionMs: 300,
        scope: 'mood',
      );

      verify(() => dio.put(
            'api/nodes/color',
            data: {
              'node_id': 'node-1',
              'rgb': {'r': 255, 'g': 128, 'b': 32},
              'brightness': 12,
              'transition_ms': 300,
              'scope': 'mood',
            },
            queryParameters: null,
          )).called(1);
    });

    test('nodeColor can update mood scene color without brightness', () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            queryParameters: any(named: 'queryParameters'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/nodes/color'),
            statusCode: 200,
          ));

      await api.nodeColor(
        nodeId: 'room-1',
        r: 1,
        g: 2,
        b: 3,
        colorScope: RhythmNodeColorScope.mood,
      );

      verify(() => dio.put(
            'api/nodes/color',
            data: {
              'node_id': 'room-1',
              'rgb': {'r': 1, 'g': 2, 'b': 3},
              'scope': 'mood',
            },
            queryParameters: null,
          )).called(1);
    });

    test('nodeColorResult parses returned state', () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            queryParameters: any(named: 'queryParameters'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/nodes/color'),
            statusCode: 200,
            data: {
              'node_id': 'room-1',
              'rhythm_enabled': true,
              'time_offset': 0.0,
              'brightness_offset': 0.0,
              'state': 'mood',
              'color': {'r': 1, 'g': 2, 'b': 3},
            },
          ));

      final state = await api.nodeColorResult(
        nodeId: 'room-1',
        r: 1,
        g: 2,
        b: 3,
        colorScope: RhythmNodeColorScope.auto,
      );

      expect(state?.nodeId, 'room-1');
      expect(state?.state, RoomModeState.mood);
      expect(state?.color?.r, 1);
      verify(() => dio.put(
            'api/nodes/color',
            data: {
              'node_id': 'room-1',
              'rgb': {'r': 1, 'g': 2, 'b': 3},
              'scope': 'auto',
            },
            queryParameters: null,
          )).called(1);
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

  // ---------------------------------------------------------------------------
  // pairDevice
  // ---------------------------------------------------------------------------
  group('pairDevice', () {
    test('passes Hue stale-bond recovery through generic pairing params',
        () async {
      when(() => dio.post(
            any(),
            data: any(named: 'data'),
            options: any(named: 'options'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/devices/pair'),
            statusCode: 200,
            data: {
              'status': 'complete',
              'devices': const <Map<String, dynamic>>[],
            },
          ));

      await api.pairDevice(
        hubType: 'hue_ble',
        params: const {'replace_stale_bonds': true},
        sessionId: 'hue-stale-bond-recovery',
      );

      final data = verify(() => dio.post(
            'api/devices/pair',
            data: captureAny(named: 'data'),
            options: any(named: 'options'),
          )).captured.single as Map<String, dynamic>;
      expect(data, {
        'hub_type': 'hue_ble',
        'session_id': 'hue-stale-bond-recovery',
        'params': {
          'replace_stale_bonds': true,
          'session_id': 'hue-stale-bond-recovery',
        },
      });
    });

    test('preserves actionable plain-text pairing failures', () async {
      when(() => dio.post(
            any(),
            data: any(named: 'data'),
            options: any(named: 'options'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/devices/pair'),
            statusCode: 500,
            data: 'Selected Hue Bluetooth bulb is no longer quarantined; '
                'refresh the recovery list.',
          ));

      final result = await api.pairDevice(
        hubType: 'hue_ble',
        params: const {
          'replace_stale_bonds': true,
          'candidate_address': 'EA:84:C2:50:A8:65',
        },
      );

      expect(result, {
        'http_status': 500,
        'error': 'Selected Hue Bluetooth bulb is no longer quarantined; '
            'refresh the recovery list.',
      });
    });
  });

  // ---------------------------------------------------------------------------
  // unpairDevice
  // ---------------------------------------------------------------------------
  group('unpairDevice', () {
    test('posts the unpair request with an extended receive timeout', () async {
      when(() => dio.post(
            any(),
            data: any(named: 'data'),
            options: any(named: 'options'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/devices/unpair'),
            statusCode: 200,
            data: {'status': 'complete', 'device_id': 'matter-100'},
          ));

      final result = await api.unpairDevice(
        hubType: 'matter',
        deviceId: 'matter-100',
      );

      expect(result?['status'], 'complete');

      final captured = verify(() => dio.post(
            'api/devices/unpair',
            data: captureAny(named: 'data'),
            options: captureAny(named: 'options'),
          )).captured;
      expect(captured[0], {
        'hub_type': 'matter',
        'params': {'device_id': 'matter-100', 'force': false},
      });
      final options = captured[1] as Options;
      expect(options.receiveTimeout, const Duration(seconds: 90));
      expect(options.sendTimeout, const Duration(seconds: 90));
    });

    test('passes force and a custom timeout through to the request', () async {
      when(() => dio.post(
            any(),
            data: any(named: 'data'),
            options: any(named: 'options'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/devices/unpair'),
            statusCode: 200,
            data: {'status': 'complete', 'device_id': 'matter-100'},
          ));

      await api.unpairDevice(
        hubType: 'matter',
        deviceId: 'matter-100',
        force: true,
        receiveTimeout: const Duration(seconds: 5),
      );

      final captured = verify(() => dio.post(
            'api/devices/unpair',
            data: captureAny(named: 'data'),
            options: captureAny(named: 'options'),
          )).captured;
      expect(
        (captured[0] as Map<String, dynamic>)['params'],
        {'device_id': 'matter-100', 'force': true},
      );
      expect(
          (captured[1] as Options).receiveTimeout, const Duration(seconds: 5));
    });

    test('passes a normalized hub address for bridge endpoint removal',
        () async {
      when(() => dio.post(
            any(),
            data: any(named: 'data'),
            options: any(named: 'options'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/devices/unpair'),
            statusCode: 200,
            data: {'status': 'complete', 'device_id': 'hue-light-7'},
          ));

      await api.unpairDevice(
        hubType: 'hue',
        deviceId: 'hue-light-7',
        hubAddress: ' 192.0.2.10 ',
      );

      final data = verify(() => dio.post(
            'api/devices/unpair',
            data: captureAny(named: 'data'),
            options: any(named: 'options'),
          )).captured.single as Map<String, dynamic>;
      expect(data, {
        'hub_type': 'hue',
        'params': {
          'device_id': 'hue-light-7',
          'hub_address': '192.0.2.10',
          'force': false,
        },
      });
    });

    test('returns actionable timeout details on DioException', () async {
      when(() => dio.post(
            any(),
            data: any(named: 'data'),
            options: any(named: 'options'),
          )).thenThrow(DioException(
        requestOptions: RequestOptions(path: 'api/devices/unpair'),
        type: DioExceptionType.receiveTimeout,
      ));

      final result = await api.unpairDevice(
        hubType: 'matter',
        deviceId: 'matter-100',
      );

      expect(result, {
        'error':
            'Unpairing timed out. Keep the device powered on nearby and try again.',
      });
    });

    test('preserves structured server error bodies', () async {
      when(() => dio.post(
            any(),
            data: any(named: 'data'),
            options: any(named: 'options'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/devices/unpair'),
            statusCode: 400,
            data: {
              'status': 'failed',
              'error': 'Bulb release characteristic was unavailable',
            },
          ));

      final result = await api.unpairDevice(
        hubType: 'hue_ble',
        deviceId: 'hue-ble-001788010f76565a',
      );

      expect(result, {
        'status': 'failed',
        'error': 'Bulb release characteristic was unavailable',
        'http_status': 400,
      });
    });

    test('preserves successful lifecycle completion details', () async {
      when(() => dio.post(
            any(),
            data: any(named: 'data'),
            options: any(named: 'options'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/devices/unpair'),
            statusCode: 200,
            data: {
              'status': 'complete',
              'completion_scope': 'local_bond_retained',
              'warning': 'The retained bond can be re-adopted.',
            },
          ));

      final result = await api.unpairDevice(
        hubType: 'hue_ble',
        deviceId: 'hue-ble-001788010f76565a',
        force: true,
      );

      expect(result?['completion_scope'], 'local_bond_retained');
      expect(result?['warning'], 'The retained bond can be re-adopted.');
    });
  });
}
