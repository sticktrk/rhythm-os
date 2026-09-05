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
    test('nodePreferencesSet reports an accepted write', () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            queryParameters: any(named: 'queryParameters'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/nodes/preferences'),
            statusCode: 204,
          ));

      final ack = await api.nodePreferencesSet(
        nodeId: 'node-1',
        state: RoomModeState.hardOff,
      );

      expect(ack, RhythmWriteAck.accepted);
    });

    test('nodePreferencesSet reports a definitive server rejection', () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            queryParameters: any(named: 'queryParameters'),
          )).thenThrow(DioException(
        requestOptions: RequestOptions(path: 'api/nodes/preferences'),
        type: DioExceptionType.badResponse,
        response: Response(
          requestOptions: RequestOptions(path: 'api/nodes/preferences'),
          statusCode: 400,
        ),
      ));

      final ack = await api.nodePreferencesSet(
        nodeId: 'node-1',
        state: RoomModeState.hardOff,
      );

      expect(ack, RhythmWriteAck.rejected);
    });

    test('nodePreferencesSet reports transport uncertainty as indeterminate',
        () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            queryParameters: any(named: 'queryParameters'),
          )).thenThrow(DioException(
        requestOptions: RequestOptions(path: 'api/nodes/preferences'),
        type: DioExceptionType.receiveTimeout,
      ));

      final ack = await api.nodePreferencesSet(
        nodeId: 'node-1',
        state: RoomModeState.hardOff,
      );

      // The server may have committed the write before the timeout; callers
      // must not treat this as a definitive rejection.
      expect(ack, RhythmWriteAck.indeterminate);
    });

    test('nodeActionChecked distinguishes rejection from uncertainty',
        () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
          )).thenThrow(DioException(
        requestOptions: RequestOptions(path: 'api/nodes/action'),
        type: DioExceptionType.badResponse,
        response: Response(
          requestOptions: RequestOptions(path: 'api/nodes/action'),
          statusCode: 409,
        ),
      ));

      final rejected =
          await api.nodeActionChecked(nodeId: 'node-1', action: 'reset');
      expect(rejected.ack, RhythmWriteAck.rejected);
      expect(rejected.state, isNull);

      when(() => dio.put(
            any(),
            data: any(named: 'data'),
          )).thenThrow(DioException(
        requestOptions: RequestOptions(path: 'api/nodes/action'),
        type: DioExceptionType.connectionTimeout,
      ));

      final uncertain =
          await api.nodeActionChecked(nodeId: 'node-1', action: 'reset');
      expect(uncertain.ack, RhythmWriteAck.indeterminate);
      expect(uncertain.state, isNull);
    });

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

    test('roomScheduleSet sends additive room profile patch', () async {
      when(() => dio.put(any(), data: any(named: 'data'))).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/nodes/preferences'),
          statusCode: 200,
          data: {
            'nodes': [
              {
                'node_id': 'room-1',
                'rhythm_enabled': true,
                'state': 'active',
                'profile_settings': {
                  'room_schedule': {
                    'source': 'follow_time',
                    'wake_time': '07:15',
                    'sleep_time': '23:45',
                  },
                },
              },
            ],
          },
        ),
      );

      final authoritative = await api.roomScheduleSet(
        roomId: 'room-1',
        schedule: const RhythmRoomSchedule(
          source: RhythmRoomScheduleSource.followTime,
          wakeTime: '07:15',
          sleepTime: '23:45',
        ),
        requestId: 'schedule-request-1',
      );

      expect(
        authoritative?.profileSettings?.roomSchedule?.source,
        RhythmRoomScheduleSource.followTime,
      );
      verify(
        () => dio.put(
          'api/nodes/preferences',
          data: {
            'node_id': 'room-1',
            'profile_settings': {
              'room_schedule': {
                'source': 'follow_time',
                'wake_time': '07:15',
                'sleep_time': '23:45',
              },
            },
            'request_id': 'schedule-request-1',
          },
        ),
      ).called(1);
    });

    test('roomScheduleTest sends room-only preview request', () async {
      when(() => dio.put(any(), data: any(named: 'data'))).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/nodes/preferences'),
          statusCode: 200,
        ),
      );

      expect(
        await api.roomScheduleTest(
          roomId: 'room-1',
          mode: RhythmMode.sleep,
          requestId: 'schedule-test-1',
        ),
        isTrue,
      );
      verify(
        () => dio.put(
          'api/nodes/preferences',
          data: {
            'node_id': 'room-1',
            'schedule_test': 'sleep',
            'request_id': 'schedule-test-1',
          },
        ),
      ).called(1);
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

    test('assignDeviceParent sends parent_id with the mutation timeout',
        () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            options: any(named: 'options'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(
              path: 'api/devices/canonical/device-1/parent',
            ),
            statusCode: 204,
          ));

      final saved = await api.assignDeviceParent('device-1', null);

      expect(saved, isTrue);
      final options = verify(() => dio.put(
            'api/devices/canonical/device-1/parent',
            data: {'parent_id': null},
            options: captureAny(named: 'options'),
          )).captured.single as Options;
      expect(options.receiveTimeout, const Duration(seconds: 30));
    });

    test('assignDeviceParentResult parses qualified projection attention',
        () async {
      when(() => dio.put(
            any(),
            data: any(named: 'data'),
            options: any(named: 'options'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(
              path: 'api/devices/canonical/device-1/parent',
            ),
            statusCode: 200,
            data: {
              'schema_version': 1,
              'canonical_committed': true,
              'projection_status': 'attention',
            },
          ));

      final result = await api.assignDeviceParentResult('device-1', 'room-2');

      expect(result, isNotNull);
      expect(result!.canonicalCommitted, isTrue);
      expect(
        result.projectionStatus,
        RhythmRoomProjectionStatus.attention,
      );
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

    test('applyHomeScene posts the whole-home body and parses targets',
        () async {
      when(() => dio.post(any(), data: any(named: 'data')))
          .thenAnswer((_) async => Response(
                requestOptions:
                    RequestOptions(path: 'api/scenes/halloween/apply-home'),
                statusCode: 200,
                data: {
                  'scene_id': 'halloween',
                  'targets': [
                    {
                      'target_id': 'room1',
                      'affected_node_ids': ['light-node-1', 'light-node-2'],
                      'unresolved_node_ids': const [],
                    },
                    {
                      'target_id': 'room2',
                      'affected_node_ids': const [],
                      'error': 'hub is offline',
                    },
                  ],
                  'applied_target_count': 1,
                  'skipped_target_count': 2,
                  'queued': true,
                  'dispatch_count': 2,
                  'dispatch_spacing_ms': 120,
                  'estimated_dispatch_ms': 240,
                },
              ));

      final result = await api.applyHomeScene(
        sceneId: 'halloween',
        transitionMs: 1200,
        dispatchSpacingMs: 120,
        correlationId: 'home-scene-123',
      );

      expect(result?.sceneId, 'halloween');
      expect(result?.targets, hasLength(2));
      expect(result?.appliedTargetCount, 1);
      expect(result?.skippedTargetCount, 2);
      expect(result?.attemptedTargetCount, 2);
      expect(result?.queued, isTrue);
      expect(result?.dispatchCount, 2);
      expect(result?.dispatchSpacingMs, 120);
      expect(result?.estimatedDispatchMs, 240);
      expect(result?.appliedTargets.single.targetId, 'room1');
      expect(
        result?.appliedTargets.single.affectedNodeIds,
        ['light-node-1', 'light-node-2'],
      );
      expect(result?.failedTargets.single.targetId, 'room2');
      expect(result?.failedTargets.single.error, 'hub is offline');
      verify(() => dio.post(
            'api/scenes/halloween/apply-home',
            data: {
              'transition_ms': 1200,
              'dispatch_spacing_ms': 120,
              'correlation_id': 'home-scene-123',
            },
          )).called(1);
    });

    test('applyHomeScene omits optional fields and tolerates a sparse response',
        () async {
      when(() => dio.post(any(), data: any(named: 'data')))
          .thenAnswer((_) async => Response(
                requestOptions:
                    RequestOptions(path: 'api/scenes/halloween/apply-home'),
                statusCode: 200,
                data: {
                  'scene_id': 'halloween',
                  'targets': [
                    {'target_id': 'room1'},
                  ],
                },
              ));

      final result = await api.applyHomeScene(sceneId: 'halloween');

      expect(result?.sceneId, 'halloween');
      // A previous-server payload without the counters still reports the
      // applied targets it did return.
      expect(result?.appliedTargetCount, 1);
      expect(result?.skippedTargetCount, 0);
      expect(result?.queued, isFalse);
      expect(result?.dispatchCount, 0);
      expect(result?.targets.single.affectedNodeIds, isEmpty);
      expect(result?.targets.single.succeeded, isTrue);
      verify(() => dio.post(
            'api/scenes/halloween/apply-home',
            data: const <String, dynamic>{},
          )).called(1);
    });

    test('applyHomeScene returns null when the server rejects the call',
        () async {
      when(() => dio.post(any(), data: any(named: 'data'))).thenThrow(
        DioException(
          requestOptions:
              RequestOptions(path: 'api/scenes/halloween/apply-home'),
          response: Response(
            requestOptions:
                RequestOptions(path: 'api/scenes/halloween/apply-home'),
            statusCode: 404,
          ),
        ),
      );

      final result = await api.applyHomeScene(sceneId: 'halloween');

      expect(result, isNull);
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
        'request_delivery': 'accepted_or_unknown',
        'error': 'Selected Hue Bluetooth bulb is no longer quarantined; '
            'refresh the recovery list.',
      });
    });

    test('classifies connection failure as accepted or unknown', () async {
      when(() => dio.post(
            any(),
            data: any(named: 'data'),
            options: any(named: 'options'),
          )).thenThrow(DioException(
        requestOptions: RequestOptions(path: 'api/devices/pair'),
        type: DioExceptionType.connectionError,
      ));

      final result = await api.pairDevice(
        hubType: 'local_ble',
        sessionId: 'local-pair-not-sent',
      );

      expect(result?['request_delivery'], 'accepted_or_unknown');
    });

    test('classifies receive timeout as accepted or unknown', () async {
      when(() => dio.post(
            any(),
            data: any(named: 'data'),
            options: any(named: 'options'),
          )).thenThrow(DioException(
        requestOptions: RequestOptions(path: 'api/devices/pair'),
        type: DioExceptionType.receiveTimeout,
      ));

      final result = await api.pairDevice(
        hubType: 'local_ble',
        sessionId: 'local-pair-timeout',
      );

      expect(result?['request_delivery'], 'accepted_or_unknown');
    });

    test('marks a structured response on connection failure as ambiguous',
        () async {
      final requestOptions = RequestOptions(path: 'api/devices/pair');
      when(() => dio.post(
            any(),
            data: any(named: 'data'),
            options: any(named: 'options'),
          )).thenThrow(DioException(
        requestOptions: requestOptions,
        response: Response(
          requestOptions: requestOptions,
          data: {
            'hub_type': 'local_ble',
            'status': 'complete',
            'device': {
              'device_id': 'partial-device',
              'name': 'Button',
              'device_type': 'button',
            },
          },
        ),
        type: DioExceptionType.connectionError,
      ));

      final result = await api.pairDevice(
        hubType: 'local_ble',
        sessionId: 'local-pair-partial-response',
      );

      expect(result?['status'], 'complete');
      expect(result?['http_status'], isNull);
      expect(result?['request_delivery'], 'accepted_or_unknown');
    });
  });

  group('getPairingResult', () {
    test('parses a durable terminal result including warnings', () async {
      when(() => dio.get(
            any(),
            options: any(named: 'options'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(
              path: 'api/devices/pair/local-pair-1',
            ),
            statusCode: 200,
            data: {
              'session_id': 'local-pair-1',
              'state': 'terminal',
              'hub_type': 'local_ble',
              'result': {
                'hub_type': 'local_ble',
                'status': 'complete',
                'device': {
                  'device_id': 'local-ble-opaque',
                  'name': 'Button',
                  'device_type': 'button',
                },
                'warnings': ['Durability acknowledgement is degraded'],
              },
            },
          ));

      final status = await api.getPairingResult('local-pair-1');

      expect(status?.state, RhythmPairingResultState.terminal);
      expect(status?.result?.succeeded, isTrue);
      expect(
          status?.result?.completedDevices.single.deviceId, 'local-ble-opaque');
      expect(
          status?.result?.warnings, ['Durability acknowledgement is degraded']);
      verify(() => dio.get(
            'api/devices/pair/local-pair-1',
            options: any(named: 'options'),
          )).called(1);
    });

    test('returns a typed not-found polling state', () async {
      when(() => dio.get(
            any(),
            options: any(named: 'options'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(
              path: 'api/devices/pair/unknown-pair',
            ),
            statusCode: 404,
            data: {
              'session_id': 'unknown-pair',
              'state': 'not_found',
            },
          ));

      final status = await api.getPairingResult('unknown-pair');

      expect(status?.state, RhythmPairingResultState.notFound);
      expect(status?.result, isNull);
    });

    test('rejects a terminal body returned with HTTP 404', () async {
      when(() => dio.get(
            any(),
            options: any(named: 'options'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(
              path: 'api/devices/pair/local-pair-http-mismatch',
            ),
            statusCode: 404,
            data: {
              'session_id': 'local-pair-http-mismatch',
              'state': 'terminal',
              'hub_type': 'local_ble',
              'result': {
                'hub_type': 'local_ble',
                'status': 'complete',
                'device': {
                  'device_id': 'local-ble-opaque',
                  'name': 'Button',
                  'device_type': 'button',
                },
              },
            },
          ));

      expect(
        await api.getPairingResult('local-pair-http-mismatch'),
        isNull,
      );
    });

    test('rejects a not-found body returned with HTTP 200', () async {
      when(() => dio.get(
            any(),
            options: any(named: 'options'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(
              path: 'api/devices/pair/local-pair-http-mismatch',
            ),
            statusCode: 200,
            data: {
              'session_id': 'local-pair-http-mismatch',
              'state': 'not_found',
            },
          ));

      expect(
        await api.getPairingResult('local-pair-http-mismatch'),
        isNull,
      );
    });

    test('rejects a response for a different session ID', () async {
      when(() => dio.get(
            any(),
            options: any(named: 'options'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(
              path: 'api/devices/pair/local-pair-expected',
            ),
            statusCode: 404,
            data: {
              'session_id': 'local-pair-other',
              'state': 'not_found',
            },
          ));

      expect(await api.getPairingResult('local-pair-expected'), isNull);
    });

    test('rejects unknown states and nonterminal terminal records', () async {
      when(() => dio.get(
            any(),
            options: any(named: 'options'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(
              path: 'api/devices/pair/local-pair-malformed',
            ),
            statusCode: 200,
            data: {
              'session_id': 'local-pair-malformed',
              'state': 'terminal',
              'hub_type': 'local_ble',
              'result': {
                'hub_type': 'local_ble',
                'status': 'searching',
              },
            },
          ));

      expect(await api.getPairingResult('local-pair-malformed'), isNull);

      when(() => dio.get(
            any(),
            options: any(named: 'options'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(
              path: 'api/devices/pair/local-pair-malformed',
            ),
            statusCode: 200,
            data: {
              'session_id': 'local-pair-malformed',
              'state': 'future_state',
            },
          ));

      expect(await api.getPairingResult('local-pair-malformed'), isNull);
    });

    test('rejects terminal records whose hub or device shape is inconsistent',
        () async {
      when(() => dio.get(
            any(),
            options: any(named: 'options'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(
              path: 'api/devices/pair/local-pair-inconsistent',
            ),
            statusCode: 200,
            data: {
              'session_id': 'local-pair-inconsistent',
              'state': 'terminal',
              'hub_type': 'local_ble',
              'result': {
                'hub_type': 'matter',
                'status': 'complete',
                'device': {
                  'device_id': '',
                  'name': 'Button',
                  'device_type': 'button',
                },
              },
            },
          ));

      expect(await api.getPairingResult('local-pair-inconsistent'), isNull);
    });

    test('rejects inconsistent singular and list device projections', () async {
      when(() => dio.get(
            any(),
            options: any(named: 'options'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(
              path: 'api/devices/pair/local-pair-device-mismatch',
            ),
            statusCode: 200,
            data: {
              'session_id': 'local-pair-device-mismatch',
              'state': 'terminal',
              'hub_type': 'local_ble',
              'result': {
                'hub_type': 'local_ble',
                'status': 'complete',
                'device': {
                  'device_id': 'local-ble-first',
                  'name': 'First button',
                  'device_type': 'button',
                },
                'devices': [
                  {
                    'device_id': 'local-ble-other',
                    'name': 'Other button',
                    'device_type': 'button',
                  },
                ],
              },
            },
          ));

      expect(
        await api.getPairingResult('local-pair-device-mismatch'),
        isNull,
      );
    });

    test('rejects terminal fields that contradict the terminal status',
        () async {
      final invalidResults = <Map<String, dynamic>>[
        {
          'hub_type': 'local_ble',
          'status': 'complete',
          'device': {
            'device_id': 'local-ble-complete-with-error',
            'name': 'Button',
            'device_type': 'button',
          },
          'error': 'contradictory error',
        },
        {
          'hub_type': 'local_ble',
          'status': 'failed',
          'device': {
            'device_id': 'local-ble-failed-with-device',
            'name': 'Button',
            'device_type': 'button',
          },
          'error': 'failed',
        },
        {
          'hub_type': 'local_ble',
          'status': 'failed',
          'error': 'failed',
          'warnings': ['   '],
        },
      ];
      var responseIndex = 0;
      when(() => dio.get(
            any(),
            options: any(named: 'options'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(
              path: 'api/devices/pair/local-pair-state-shape',
            ),
            statusCode: 200,
            data: {
              'session_id': 'local-pair-state-shape',
              'state': 'terminal',
              'hub_type': 'local_ble',
              'result': invalidResults[responseIndex++],
            },
          ));

      for (var index = 0; index < invalidResults.length; index += 1) {
        expect(
          await api.getPairingResult('local-pair-state-shape'),
          isNull,
        );
      }
    });
  });

  group('acknowledgePairingResult', () {
    test('returns true only after the server confirms HTTP 204', () async {
      when(() => dio.delete(
            any(),
            options: any(named: 'options'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(
              path: 'api/devices/pair/local-pair-ack',
            ),
            statusCode: 204,
          ));

      expect(await api.acknowledgePairingResult('local-pair-ack'), isTrue);
      verify(() => dio.delete(
            'api/devices/pair/local-pair-ack',
            options: any(named: 'options'),
          )).called(1);
    });

    test('returns false when the server does not acknowledge', () async {
      when(() => dio.delete(
            any(),
            options: any(named: 'options'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(
              path: 'api/devices/pair/local-pair-pending',
            ),
            statusCode: 409,
            data: {'error': 'Pairing result is still pending'},
          ));

      expect(
        await api.acknowledgePairingResult('local-pair-pending'),
        isFalse,
      );
    });

    test('returns false when acknowledgement transport fails', () async {
      when(() => dio.delete(
            any(),
            options: any(named: 'options'),
          )).thenThrow(DioException(
        requestOptions: RequestOptions(
          path: 'api/devices/pair/local-pair-offline',
        ),
        type: DioExceptionType.connectionError,
      ));

      expect(
        await api.acknowledgePairingResult('local-pair-offline'),
        isFalse,
      );
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

    test('adds archive only when the negotiated archive path is requested',
        () async {
      when(() => dio.post(
            any(),
            data: any(named: 'data'),
            options: any(named: 'options'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/devices/unpair'),
            statusCode: 200,
            data: {'status': 'complete'},
          ));

      await api.unpairDevice(
        hubType: 'matter',
        deviceId: 'matter-100',
        archive: true,
      );

      final data = verify(() => dio.post(
            'api/devices/unpair',
            data: captureAny(named: 'data'),
            options: any(named: 'options'),
          )).captured.single as Map<String, dynamic>;
      expect(data['params'], {
        'device_id': 'matter-100',
        'force': false,
        'archive': true,
      });
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

    test('passes bounded lifecycle correlation metadata for removal', () async {
      when(() => dio.post(
            any(),
            data: any(named: 'data'),
            options: any(named: 'options'),
          )).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/devices/unpair'),
            statusCode: 200,
            data: {'status': 'complete'},
          ));

      await api.unpairDevice(
        hubType: 'hue',
        deviceId: 'hue-switch-7',
        deviceType: ' button ',
        correlationId: ' hue-bridge-remove-journey ',
      );

      final data = verify(() => dio.post(
            'api/devices/unpair',
            data: captureAny(named: 'data'),
            options: any(named: 'options'),
          )).captured.single as Map<String, dynamic>;
      expect(data['params'], {
        'device_id': 'hue-switch-7',
        'device_type': 'button',
        'correlation_id': 'hue-bridge-remove-journey',
        'force': false,
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

  group('removed devices', () {
    test('lists archived device metadata without transformation', () async {
      when(() => dio.get(any())).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/devices/removed'),
            statusCode: 200,
            data: [
              {
                'id': 'canonical-42',
                'name': 'Desk bulb',
                'removed_at': 1234,
                'recovery_available': true,
              },
            ],
          ));

      final result = await api.getRemovedDevices();

      expect(result, hasLength(1));
      expect(result!.single['id'], 'canonical-42');
      expect(result.single['recovery_available'], isTrue);
      verify(() => dio.get('api/devices/removed')).called(1);
    });

    test('permanent delete sends bounded correlation header', () async {
      when(() => dio.delete(
            any(),
            options: any(named: 'options'),
          )).thenAnswer((_) async => Response(
            requestOptions:
                RequestOptions(path: 'api/devices/canonical/canonical-42'),
            statusCode: 204,
          ));

      expect(
        await api.permanentlyDeleteRemovedDevice(
          'canonical-42',
          correlationId: ' removed-bulb-purge-42 ',
        ),
        isTrue,
      );

      final options = verify(() => dio.delete(
            'api/devices/canonical/canonical-42',
            options: captureAny(named: 'options'),
          )).captured.single as Options;
      expect(options.headers?['X-Request-Id'], 'removed-bulb-purge-42');
    });
  });

  group('Hue room authority', () {
    final payload = {
      'schema_version': 1,
      'bridges': [
        {
          'address': 'bridge.local',
          'revision': '0123456789abcdef',
          'takeover_scope': 'bridge',
          'bridge_takeover_requested': false,
          'rooms': [
            {
              'room_id': 'office',
              'name': 'Office',
              'owner': 'unreviewed',
              'rhythm_automation_enabled': false,
            },
          ],
        },
      ],
    };

    test('fetches the complete review', () async {
      when(() => dio.get(any())).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/hue/authority'),
            statusCode: 200,
            data: payload,
          ));

      final result = await api.getHueAuthority();

      expect(result?.bridges.single.rooms.single.name, 'Office');
      verify(() => dio.get('api/hue/authority')).called(1);
    });

    test('submits every room and defaults unreviewed rooms to Hue', () async {
      when(() => dio.put(any(), data: any(named: 'data')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(path: 'api/hue/authority'),
                statusCode: 200,
                data: payload,
              ));
      final bridge = RhythmHueAuthority.fromJson(payload).bridges.single;

      final result = await api.updateHueAuthority(
        bridge: bridge,
        owners: const {},
        correlationId: 'hue-authority-test',
        topologySyncEnabled: true,
      );

      expect(result, isNotNull);
      verify(() => dio.put('api/hue/authority', data: {
            'address': 'bridge.local',
            'revision': '0123456789abcdef',
            'correlation_id': 'hue-authority-test',
            'topology_sync_enabled': true,
            'rooms': [
              {'room_id': 'office', 'owner': 'hue'},
            ],
          })).called(1);
    });
  });

  group('named light schedules', () {
    test('fetches reusable schedule definitions', () async {
      when(() => dio.get('api/light-schedules'))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(path: 'api/light-schedules'),
                statusCode: 200,
                data: {
                  'schedules': [
                    {
                      'id': 'outdoor',
                      'name': 'Outdoor lights',
                      'enabled': true,
                      'active_mode': 'day',
                      'transitions': [],
                    },
                  ],
                },
              ));

      final schedules = await api.getLightSchedules();

      expect(schedules.single.id, 'outdoor');
      expect(schedules.single.activeMode, RhythmMode.day);
    });

    test('replaces schedules with the exact reviewed registry', () async {
      const current = RhythmLightScheduleConfig(
        id: 'outdoor',
        name: 'Outdoor lights',
        activeMode: RhythmMode.day,
      );
      const updated = RhythmLightScheduleConfig(
        id: 'outdoor',
        name: 'Porch lights',
        activeMode: RhythmMode.day,
      );
      when(() => dio.put(any(), data: any(named: 'data')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(path: 'api/light-schedules'),
                statusCode: 200,
                data: {
                  'schedules': [updated.toJson()],
                },
              ));

      final schedules = await api.setLightSchedules(
        const [updated],
        expectedSchedules: const [current],
        correlationId: 'schedule-journey-1',
      );

      expect(schedules.single.name, 'Porch lights');
      verify(() => dio.put('api/light-schedules', data: {
            'schedules': [updated.toJson()],
            'expected_schedules': [current.toJson()],
            'correlation_id': 'schedule-journey-1',
          })).called(1);
    });

    test('clears assignment with null and returns authoritative state',
        () async {
      when(() => dio.put(any(), data: any(named: 'data')))
          .thenAnswer((_) async => Response(
                requestOptions:
                    RequestOptions(path: 'api/light-schedules/assignment'),
                statusCode: 200,
                data: {
                  'nodes': [
                    {
                      'room_id': 'porch',
                      'rhythm_enabled': true,
                      'profile_settings': {
                        'light_schedule': {
                          'kind': 'unscheduled',
                          'active_mode': 'day',
                        },
                      },
                    },
                  ],
                },
              ));

      final state = await api.setLightScheduleAssignment(
        nodeId: 'porch',
        scheduleId: null,
        correlationId: 'assignment-journey-1',
      );

      expect(state?.roomId, 'porch');
      expect(state?.profileSettings?.isExplicitlyUnscheduled, isTrue);
      verify(() => dio.put('api/light-schedules/assignment', data: {
            'node_id': 'porch',
            'schedule_id': null,
            'correlation_id': 'assignment-journey-1',
          })).called(1);
    });

    test('restores legacy schedule authority explicitly', () async {
      when(() => dio.put(any(), data: any(named: 'data')))
          .thenAnswer((_) async => Response(
                requestOptions:
                    RequestOptions(path: 'api/light-schedules/assignment'),
                statusCode: 200,
                data: {
                  'nodes': [
                    {
                      'room_id': 'porch',
                      'rhythm_enabled': true,
                      'profile_settings': const <String, dynamic>{},
                    },
                  ],
                },
              ));

      final state = await api.clearLightScheduleAssignment(
        nodeId: 'porch',
        correlationId: 'legacy-journey-1',
      );

      expect(state?.roomId, 'porch');
      expect(state?.profileSettings?.lightSchedule, isNull);
      verify(() => dio.put('api/light-schedules/assignment', data: {
            'node_id': 'porch',
            'legacy': true,
            'correlation_id': 'legacy-journey-1',
          })).called(1);
    });

    test('writes one sparse override with an exact reviewed precondition',
        () async {
      when(() => dio.put(any(), data: any(named: 'data')))
          .thenAnswer((_) async => Response(
                requestOptions:
                    RequestOptions(path: 'api/light-schedules/override'),
                statusCode: 200,
                data: {
                  'nodes': [
                    {'room_id': 'porch', 'rhythm_enabled': true},
                  ],
                },
              ));
      const scheduleOverride = RhythmLightScheduleOverride(
        transitions: {
          'wake': RhythmModeTransitionOverride(
            trigger: RhythmTransitionTriggerOverride(offsetMinutes: -30),
          ),
        },
      );

      final state = await api.setLightScheduleOverride(
        nodeId: 'porch',
        scheduleId: 'outdoor',
        scheduleOverride: scheduleOverride,
        expectedEffectiveOverrides: const {'outdoor': scheduleOverride},
        correlationId: 'journey-1',
      );

      expect(state?.roomId, 'porch');
      verify(() => dio.put('api/light-schedules/override', data: {
            'node_id': 'porch',
            'schedule_id': 'outdoor',
            'override': scheduleOverride.toJson(),
            'expected_effective_overrides': {
              'outdoor': scheduleOverride.toJson(),
            },
            'correlation_id': 'journey-1',
          })).called(1);
    });
  });

  group('Matter setup code recovery', () {
    test('parses a secret-bearing response without transforming the payload',
        () async {
      when(() => dio.get(
            any(),
            options: any(named: 'options'),
          )).thenAnswer((_) async => Response(
            requestOptions:
                RequestOptions(path: 'api/matter/setup-code/matter-42-2'),
            statusCode: 200,
            data: {
              'payload_kind': 'qr_code',
              'setup_payload': 'MT:RECOVERY-SECRET',
              'captured_at': '2026-08-11T12:00:00Z',
            },
          ));

      final result = await api.getMatterSetupCode('matter-42-2');

      expect(result?.payloadKind, RhythmPairingRecoveryPayloadKind.qrCode);
      expect(result?.setupPayload, 'MT:RECOVERY-SECRET');
      expect(result?.capturedAt.toUtc(), DateTime.utc(2026, 8, 11, 12));
      verify(() => dio.get(
            'api/matter/setup-code/matter-42-2',
            options: any(named: 'options'),
          )).called(1);
    });

    test('returns null when the appliance has no saved code', () async {
      when(() => dio.get(
            any(),
            options: any(named: 'options'),
          )).thenAnswer((_) async => Response(
            requestOptions:
                RequestOptions(path: 'api/matter/setup-code/matter-42'),
            statusCode: 404,
          ));

      expect(await api.getMatterSetupCode('matter-42'), isNull);
    });
  });

  group('device attention', () {
    test('parses actionable typed entries and tolerates additive fields',
        () async {
      when(() => dio.get(any())).thenAnswer((_) async => Response(
            requestOptions: RequestOptions(path: 'api/device-attention'),
            statusCode: 200,
            data: [
              {
                'id': 'unreachable-opaque-1',
                'journey_id': 'unreachable-device-review-1',
                'kind': 'unreachable_device',
                'status': 'pending',
                'device': {
                  'name': 'Hall Lamp',
                  'native_id': 'matter-42-1',
                  'hub_type': 'matter',
                  'hub_address': 'local',
                  'device_type': 'light',
                },
                'evidence': {'failure_count': 3},
                'guidance': 'Power cycle the light.',
                'future_field': true,
              },
              {
                'id': 'missing-review-journey',
                'kind': 'unreachable_device',
                'status': 'pending',
                'device': {
                  'native_id': 'matter-43-1',
                  'hub_type': 'matter',
                },
                'evidence': const <String, dynamic>{},
              },
              {
                'id': 'future-kind',
                'journey_id': 'unreachable-device-review-2',
                'kind': 'future_attention_kind',
                'status': 'future_status',
                'device': {
                  'native_id': 'matter-44-1',
                  'hub_type': 'matter',
                },
                'evidence': const <String, dynamic>{},
              },
            ],
          ));

      final entries = await api.getDeviceAttentionEntries();

      expect(entries, hasLength(1));
      expect(entries!.single.device.name, 'Hall Lamp');
      expect(entries.single.journeyId, 'unreachable-device-review-1');
      expect(entries.single.evidence.failureCount, 3);
      verify(() => dio.get('api/device-attention')).called(1);
    });

    test('uses encoded entry ids and explicit journey correlation', () async {
      when(() => dio.put(any(), data: any(named: 'data')))
          .thenAnswer((invocation) async => Response(
                requestOptions: RequestOptions(
                  path: invocation.positionalArguments.first as String,
                ),
                statusCode: 200,
              ));

      expect(
        await api.snoozeDeviceAttention(
          'opaque/id',
          correlationId: 'journey-1',
        ),
        isTrue,
      );
      expect(
        await api.markDeviceAttentionStillInstalled(
          'opaque/id',
          correlationId: 'journey-1',
        ),
        isTrue,
      );
      expect(
        await api.markDeviceAttentionRemovalSelected(
          'opaque/id',
          correlationId: 'journey-1',
        ),
        isTrue,
      );

      for (final action in [
        'snooze',
        'still-installed',
        'removal-selected',
      ]) {
        verify(() => dio.put(
              'api/device-attention/opaque%2Fid/$action',
              data: {'correlation_id': 'journey-1'},
            )).called(1);
      }
    });

    test('returns null when the additive route is unavailable', () async {
      when(() => dio.get(any())).thenThrow(DioException(
        requestOptions: RequestOptions(path: 'api/device-attention'),
        response: Response(
          requestOptions: RequestOptions(path: 'api/device-attention'),
          statusCode: 404,
        ),
      ));

      expect(await api.getDeviceAttentionEntries(), isNull);
    });
  });

  group('Bulb Audition', () {
    test('runs a canonical scenario with its correlation and override',
        () async {
      when(() => dio.post(any(), data: any(named: 'data')))
          .thenAnswer((_) async => Response(
                requestOptions: RequestOptions(path: 'api/matter/audition/run'),
                statusCode: 200,
                data: {'schema_version': 3, 'status': 'ok'},
              ));

      final result = await api.runBulbAudition(
        deviceId: 'matter-42-2',
        scenario: 'try_with',
        journeyId: 'bulb-audition-1',
        baseScenario: 'turn_on_from_off',
        profileOverride: {'turn_on': 'explicit_on_first'},
      );

      expect(result?['schema_version'], 3);
      verify(() => dio.post('api/matter/audition/run', data: {
            'device_id': 'matter-42-2',
            'scenario': 'try_with',
            'journey_id': 'bulb-audition-1',
            'base_scenario': 'turn_on_from_off',
            'profile_override': {'turn_on': 'explicit_on_first'},
          })).called(1);
    });

    test('always saves the canonical report as schema v3', () async {
      when(() => dio.post(any(), data: any(named: 'data')))
          .thenAnswer((_) async => Response(
                requestOptions:
                    RequestOptions(path: 'api/matter/audition/report'),
                statusCode: 200,
                data: {'status': 'saved'},
              ));

      final result = await api.saveBulbAuditionReport(
        {'schema_version': 2, 'report_id': 'report-1'},
        applyLocal: false,
      );

      expect(result?['status'], 'saved');
      verify(() => dio.post('api/matter/audition/report', data: {
            'schema_version': 3,
            'report_id': 'report-1',
            'apply_local': false,
          })).called(1);
    });
  });
}
