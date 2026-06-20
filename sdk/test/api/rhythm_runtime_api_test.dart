import 'package:dio/dio.dart';
import 'package:mocktail/mocktail.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

import '../helpers/mock_dio.dart';

void main() {
  late MockDio dio;
  late List<List<RhythmRoomState>> cacheUpdates;
  late RhythmRuntimeApi api;

  setUp(() {
    dio = MockDio();
    cacheUpdates = [];
    api = RhythmRuntimeApi(dio, onStatesReceived: (states) {
      cacheUpdates.add(states);
    });
  });

  group('node runtime reads', () {
    test('getNodesState parses node poll envelope and caches states', () async {
      when(() => dio.get('api/nodes/state')).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/nodes/state'),
          statusCode: 200,
          data: {
            'hub_connected': true,
            'nodes': [
              {
                'id': 'room-1',
                'name': 'Living Room',
                'kind': 'room',
                'state': 'active',
                'rhythm_enabled': true,
                'time_offset': 0.0,
                'brightness_offset': 0.0,
                'lights_on': true,
                'brightness': 72,
                'kelvin': 3300,
              },
            ],
          },
        ),
      );

      final result = await api.getNodesState();

      expect(result.hubConnected, isTrue);
      expect(result.nodes, hasLength(1));
      expect(result.nodes.single.nodeId, 'room-1');
      expect(result.nodes.single.kind, RhythmNodeKind.room);
      expect(cacheUpdates, hasLength(1));
      expect(cacheUpdates.single.single.brightness, 72);
    });

    test('getNodeNow parses node plus pipeline trace', () async {
      when(() => dio.get('api/nodes/room-1/now')).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/nodes/room-1/now'),
          statusCode: 200,
          data: {
            'node': {
              'id': 'room-1',
              'name': 'Living Room',
              'kind': 'room',
              'state': 'active',
              'rhythm_enabled': true,
              'time_offset': 0.0,
              'brightness_offset': 0.0,
              'lights_on': true,
              'brightness': 44,
              'kelvin': 2800,
            },
            'pipeline_trace': {
              'entries': [
                {
                  'stage_id': 'base_curve',
                  'phase': 'render',
                  'value': 'brightness',
                  'input': 0.2,
                  'output': 0.44,
                },
              ],
              'dispatch_decisions': [
                {
                  'stage_id': 'final',
                  'target_id': 'room-1',
                  'should_dispatch': true,
                  'reason': 'changed',
                },
              ],
            },
          },
        ),
      );

      final result = await api.getNodeNow('room-1');

      expect(result, isNotNull);
      expect(result!.node.brightness, 44);
      expect(result.pipelineTrace.entries.single.stageId, 'base_curve');
      expect(
          result.pipelineTrace.dispatchDecisions.single.shouldDispatch, isTrue);
      expect(cacheUpdates.single.single.nodeId, 'room-1');
    });

    test('previewNodeSlider posts node path and parses dispatch metadata',
        () async {
      when(() => dio.post(any(), data: any(named: 'data'))).thenAnswer(
        (_) async => Response(
          requestOptions:
              RequestOptions(path: 'api/nodes/room-1/slider-preview'),
          statusCode: 200,
          data: {
            'nodes': [
              {
                'id': 'room-1',
                'rhythm_enabled': true,
                'time_offset': 15.0,
                'brightness_offset': 0.0,
                'state': 'active',
              },
            ],
            'queued': true,
            'dispatch_count': 1,
          },
        ),
      );

      final result = await api.previewNodeSlider(
        nodeId: 'room-1',
        timeOffset: 15.0,
        brightness: 50,
        colorTemperature: 3200,
        preserveBrightness: true,
      );

      expect(result.states.single.nodeId, 'room-1');
      expect(result.queued, isTrue);
      verify(() => dio.post(
            'api/nodes/room-1/slider-preview',
            data: {
              'time_offset': 15.0,
              'brightness': 50,
              'color_temperature': 3200,
              'preserve_brightness': true,
            },
          )).called(1);
    });
  });

  group('scope-layer writes', () {
    test('freezeNode posts timed freeze request and parses applied writes',
        () async {
      when(() => dio.put('api/nodes/freeze', data: any(named: 'data')))
          .thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/nodes/freeze'),
          statusCode: 200,
          data: {
            'nodes': [
              {
                'node_id': 'room-1',
                'writes': [
                  {
                    'kind': 'merge_runtime',
                    'fields': {
                      'freeze': {
                        'frozen_at_hour': 2.5,
                        'expires_at_epoch_ms': 9000,
                      },
                    },
                  },
                ],
              },
            ],
          },
        ),
      );

      final result = await api.freezeNode(
        nodeId: 'room-1',
        frozenAtHour: 2.5,
        expiresAtEpochMs: 9000,
      );

      expect(result.nodes.single.nodeId, 'room-1');
      expect(result.nodes.single.writes.single['kind'], 'merge_runtime');
      verify(() => dio.put(
            'api/nodes/freeze',
            data: [
              {
                'node_id': 'room-1',
                'frozen_at_hour': 2.5,
                'expires_at_epoch_ms': 9000,
              },
            ],
          )).called(1);
    });

    test('boost and auto-off convenience methods use Rust timed-state routes',
        () async {
      when(() => dio.put(any(), data: any(named: 'data'))).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/nodes/boost'),
          statusCode: 200,
          data: {'nodes': const []},
        ),
      );

      await api.boostNode(
        nodeId: 'room-1',
        brightness: 18.0,
        expiresInSecs: 30,
      );
      await api.armNodeAutoOff(
        nodeId: 'room-1',
        expiresInSecs: 120,
        mode: 'max_extend',
      );
      await api.clearNodeBoost('room-1');

      verify(() => dio.put(
            'api/nodes/boost',
            data: [
              {
                'node_id': 'room-1',
                'brightness': 18.0,
                'expires_in_secs': 30,
              },
            ],
          )).called(1);
      verify(() => dio.put(
            'api/nodes/auto-off',
            data: [
              {
                'node_id': 'room-1',
                'expires_in_secs': 120,
                'mode': 'max_extend',
              },
            ],
          )).called(1);
      verify(() => dio.put(
            'api/nodes/boost',
            data: [
              {'node_id': 'room-1', 'clear': true},
            ],
          )).called(1);
    });
  });

  group('environment', () {
    test('setOutdoorOverride uses current Rust condition contract', () async {
      when(() => dio.put('api/outdoor/override', data: any(named: 'data')))
          .thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/outdoor/override'),
          statusCode: 200,
          data: {
            'outdoor_factor': 0.4,
            'source': 'manual_override',
            'is_fallback': false,
            'diagnostics': {
              'sun_position': 0.7,
              'sun_elevation_degrees': 25.0,
              'sun_angle_factor': 0.7,
              'sky_condition': 'rain',
              'sky_multiplier': 0.35,
              'unavailable_sources': ['lux_sensor'],
            },
          },
        ),
      );

      final result = await api.setOutdoorOverride(
        condition: RhythmSkyCondition.rain,
        expiresInSecs: 600,
      );

      expect(result!.source, RhythmEnvironmentSource.manualOverride);
      expect(result.diagnostics.skyCondition, RhythmSkyCondition.rain);
      verify(() => dio.put(
            'api/outdoor/override',
            data: {
              'condition': 'rain',
              'expires_in_secs': 600,
            },
          )).called(1);
    });

    test('learnEnvironmentBaselines parses calibration response', () async {
      when(() => dio.post(
            'api/environment/learn-baselines',
            data: any(named: 'data'),
          )).thenAnswer(
        (_) async => Response(
          requestOptions:
              RequestOptions(path: 'api/environment/learn-baselines'),
          statusCode: 200,
          data: {
            'learned': true,
            'calibration': {
              'lux_learned_floor': 12.0,
              'lux_learned_ceiling': 900.0,
            },
          },
        ),
      );

      final result = await api.learnEnvironmentBaselines([12, 900]);

      expect(result!.learned, isTrue);
      expect(result.calibration.luxLearnedCeiling, 900);
      verify(() => dio.post(
            'api/environment/learn-baselines',
            data: {
              'lux_samples': [12.0, 900.0],
            },
          )).called(1);
    });
  });

  group('runtime catalogs and documents', () {
    test('getInputActions parses action catalog', () async {
      when(() => dio.get('api/input-actions')).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/input-actions'),
          statusCode: 200,
          data: {
            'actions': [
              {
                'python_action_id': 'toggle',
                'migrated_id': 'toggle',
                'category': 'power',
                'label': 'Toggle',
                'rust_action': 'toggle',
                'parameters': [
                  {'key': 'mode', 'value': 'toggle'},
                ],
                'allowed_target_grains': ['room'],
                'supports_when_off': true,
                'allowed_when_off_values': ['on'],
                'dispatch_precondition': 'always',
                'import_behavior': 'migrated',
              },
            ],
            'dynamic_patterns': [
              {
                'pattern': 'brightness_*',
                'rust_action': 'brightness',
                'param_template': 'brightness',
                'allowed_target_grains': ['room', 'device'],
                'dispatch_precondition': 'target_on',
                'import_behavior': 'dynamic',
              },
            ],
          },
        ),
      );

      final result = await api.getInputActions();

      expect(result.actions.single.rustAction, 'toggle');
      expect(result.actions.single.parameters.single.key, 'mode');
      expect(result.dynamicPatterns.single.pattern, 'brightness_*');
    });

    test('power schedules, history, and filter presets parse wrappers',
        () async {
      when(() => dio.get('api/nodes/power-schedule')).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/nodes/power-schedule'),
          statusCode: 200,
          data: {
            'schedules': [
              {
                'id': 'wake',
                'node_id': 'room-1',
                'action': 'on',
                'slot': {
                  'kind': 'clock',
                  'time': {'hour': 7, 'minute': 30},
                },
              },
            ],
          },
        ),
      );
      when(() => dio.get('api/history')).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/history'),
          statusCode: 200,
          data: {
            'activities': [
              {
                'id': 'evt-1',
                'node_id': 'room-1',
                'action_id': 'toggle',
                'source': {
                  'raw': 'webserver',
                  'kind': 'app',
                  'marks_touched': true,
                },
                'epoch_ms': 1778058932588,
                'count': 2,
              },
            ],
          },
        ),
      );
      when(() => dio.get('api/filter-presets')).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/filter-presets'),
          statusCode: 200,
          data: {
            'schema_version': 1,
            'presets': [
              {
                'id': 'desk',
                'filter': {
                  'device_ids': ['lamp-a'],
                },
              },
            ],
          },
        ),
      );

      final schedules = await api.getPowerSchedules();
      final history = await api.getHistory();
      final presets = await api.getFilterPresets();

      expect(schedules.schedules.single['node_id'], 'room-1');
      expect(history.activities.single.source.kind, 'app');
      expect(history.activities.single.count, 2);
      expect(presets.presets.single['id'], 'desk');
    });
  });

  group('integration sync and reports', () {
    test('posts Rust integration sync and device report routes', () async {
      when(() => dio.post('api/integrations/sync-devices')).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/integrations/sync-devices'),
          statusCode: 200,
          data: {
            'rooms_added': 1,
            'rooms_updated': 2,
            'rooms_removed': 0,
            'devices_synced': 3,
          },
        ),
      );
      when(() => dio.post('api/integrations/sync-controls')).thenAnswer(
        (_) async => Response(
          requestOptions:
              RequestOptions(path: 'api/integrations/sync-controls'),
          statusCode: 200,
          data: {
            'requested_by': 'sync_controls',
            'scheduled': true,
          },
        ),
      );
      when(
        () => dio.post(
          'api/devices/report',
          data: any(named: 'data'),
        ),
      ).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/devices/report'),
          statusCode: 200,
          data: {
            'canonical_devices': [
              {'id': 'matter-1'},
            ],
          },
        ),
      );

      final deviceSync = await api.syncIntegrationDevices();
      final controlSync = await api.syncIntegrationControls();
      final report = await api.reportDevices({
        'devices': [
          {'id': 'matter-1'},
        ],
      });

      expect(deviceSync!['devices_synced'], 3);
      expect(controlSync!['scheduled'], isTrue);
      expect(report!['canonical_devices'], hasLength(1));
      verify(() => dio.post('api/integrations/sync-devices')).called(1);
      verify(() => dio.post('api/integrations/sync-controls')).called(1);
      verify(
        () => dio.post(
          'api/devices/report',
          data: {
            'devices': [
              {'id': 'matter-1'},
            ],
          },
        ),
      ).called(1);
    });
  });

  group('controls', () {
    test('control pause and hardware settings use control routes', () async {
      when(() => dio.put(
            'api/controls/button-1/pause',
            data: any(named: 'data'),
          )).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/controls/button-1/pause'),
          statusCode: 200,
          data: {
            'control_id': 'button-1',
            'active': true,
            'expires_at_epoch_ms': 1778058932588,
          },
        ),
      );
      when(() => dio.get('api/controls/button-1/hardware-settings')).thenAnswer(
        (_) async => Response(
          requestOptions:
              RequestOptions(path: 'api/controls/button-1/hardware-settings'),
          statusCode: 200,
          data: {
            'supported': true,
            'control_id': 'button-1',
            'settings': {'led': 'low'},
          },
        ),
      );

      final pause = await api.setControlPause('button-1', expiresInSecs: 300);
      final settings = await api.getControlHardwareSettings('button-1');

      expect(pause!.active, isTrue);
      expect(settings!['supported'], isTrue);
      verify(() => dio.put(
            'api/controls/button-1/pause',
            data: {'expires_in_secs': 300},
          )).called(1);
    });
  });
}
