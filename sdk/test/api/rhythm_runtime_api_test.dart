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
  });

  group('history', () {
    test('getHistory parses activity wrapper', () async {
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

      final history = await api.getHistory();

      expect(history.activities.single.source.kind, 'app');
      expect(history.activities.single.count, 2);
    });

    test('getHistory sends optional server-side activity filters', () async {
      when(() => dio.get(
            'api/history',
            queryParameters: any(named: 'queryParameters'),
          )).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/history'),
          statusCode: 200,
          data: {'activities': const []},
        ),
      );

      await api.getHistory(
        limit: 5000,
        area: 'room-1',
        source: 'physical_button',
        action: 'turn_on',
      );

      verify(() => dio.get(
            'api/history',
            queryParameters: {
              'limit': 5000,
              'area': 'room-1',
              'source': 'physical_button',
              'action': 'turn_on',
            },
          )).called(1);
    });
  });

  group('activity cloud', () {
    test('activity cloud config uses device provisioning routes', () async {
      when(() => dio.get('api/activity-cloud/config')).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/activity-cloud/config'),
          statusCode: 200,
          data: {
            'configured': true,
            'hub_id': 'hub-1',
            'home_id': 'home-1',
          },
        ),
      );
      when(() => dio.put(
            'api/activity-cloud/config',
            data: any(named: 'data'),
          )).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/activity-cloud/config'),
          statusCode: 200,
          data: {'configured': true},
        ),
      );
      when(() => dio.delete('api/activity-cloud/config')).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/activity-cloud/config'),
          statusCode: 200,
          data: {'configured': false},
        ),
      );

      final status = await api.getActivityCloudConfig();
      final updated = await api.putActivityCloudConfig({
        'ingest_url':
            'https://example.test/functions/v1/server-activity-ingest',
        'upload_token': 'token',
        'home_id': 'home-1',
        'hub_id': 'hub-1',
      });
      final cleared = await api.clearActivityCloudConfig();

      expect(status?['hub_id'], 'hub-1');
      expect(updated?['configured'], isTrue);
      expect(cleared?['configured'], isFalse);
      verify(() => dio.put(
            'api/activity-cloud/config',
            data: {
              'ingest_url':
                  'https://example.test/functions/v1/server-activity-ingest',
              'upload_token': 'token',
              'home_id': 'home-1',
              'hub_id': 'hub-1',
            },
          )).called(1);
    });
  });
}
