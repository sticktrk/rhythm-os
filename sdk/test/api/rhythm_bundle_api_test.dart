import 'package:dio/dio.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:mocktail/mocktail.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

import '../helpers/mock_dio.dart';

void main() {
  late MockDio dio;
  late RhythmBundleApi api;

  setUp(() {
    dio = MockDio();
    api = RhythmBundleApi(baseUrl: 'http://test/', dio: dio);

    registerFallbackValue(Options());
  });

  group('getConfigurationBundle', () {
    test('parses object responses', () async {
      when(
        () => dio.get(
          any(),
          options: any(named: 'options'),
        ),
      ).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/profile-bundle'),
          statusCode: 200,
          data: {'kind': 'profile_bundle'},
        ),
      );

      final result = await api.getConfigurationBundle();

      expect(result, {'kind': 'profile_bundle'});
      final captured = verify(
        () => dio.get(
          captureAny(),
          options: captureAny(named: 'options'),
        ),
      ).captured;
      expect(captured[0], 'api/profile-bundle');
      expect((captured[1] as Options).validateStatus?.call(500), isTrue);
    });
  });

  group('portable lighting settings', () {
    test(
      'preview delegates conversion without writing the installation',
      () async {
        when(
          () => dio.post(
            any(),
            data: any(named: 'data'),
            options: any(named: 'options'),
          ),
        ).thenAnswer(
          (_) async => Response(
            requestOptions: RequestOptions(
              path: 'api/lighting-settings/preview',
            ),
            statusCode: 200,
            data: {'kind': 'lighting_settings', 'nodes': []},
          ),
        );
        final result = await api.previewLightingSettings({
          'kind': 'backup_bundle',
        });
        expect(result['kind'], 'lighting_settings');
        verify(
          () => dio.post(
            'api/lighting-settings/preview',
            data: {'kind': 'backup_bundle'},
            options: any(named: 'options'),
          ),
        ).called(1);
        verifyNever(
          () => dio.put(
            any(),
            data: any(named: 'data'),
            options: any(named: 'options'),
          ),
        );
      },
    );

    test(
      'restore sends reviewed mapping and keeps skipped nodes unmapped',
      () async {
        when(
          () => dio.put(
            any(),
            data: any(named: 'data'),
            options: any(named: 'options'),
          ),
        ).thenAnswer(
          (_) async => Response(
            requestOptions: RequestOptions(path: 'api/lighting-settings'),
            statusCode: 200,
            data: {'applied_nodes': 1, 'skipped_nodes': 1},
          ),
        );
        final result = await api.putLightingSettings(
          {'kind': 'lighting_settings'},
          nodeMappings: {'old-room': 'ha-room'},
        );
        expect(result['skipped_nodes'], 1);
        verify(
          () => dio.put(
            'api/lighting-settings',
            data: {
              'settings': {'kind': 'lighting_settings'},
              'node_mappings': {'old-room': 'ha-room'},
            },
            options: any(named: 'options'),
          ),
        ).called(1);
      },
    );

    test('rejected restore is surfaced without retry', () async {
      when(
        () => dio.put(
          any(),
          data: any(named: 'data'),
          options: any(named: 'options'),
        ),
      ).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/lighting-settings'),
          statusCode: 400,
          data: {'error': 'Destination no longer exists'},
        ),
      );
      await expectLater(
        api.putLightingSettings(
          {'kind': 'lighting_settings'},
          nodeMappings: {'old': 'missing'},
        ),
        throwsA(isA<RhythmException>()),
      );
      verify(
        () => dio.put(
          any(),
          data: any(named: 'data'),
          options: any(named: 'options'),
        ),
      ).called(1);
    });
  });

  group('getDeploymentCapabilities', () {
    void state(Map<String, dynamic> data) {
      when(() => dio.get<Map<String, dynamic>>('api/state')).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/state'),
          statusCode: 200,
          data: data,
        ),
      );
    }

    test('absent capabilities preserve legacy appliance operations', () async {
      state({'version': '0.6.632', 'nodes': []});
      final deployment = await api.getDeploymentCapabilities();
      expect(deployment.fullBackupExport, isTrue);
      expect(deployment.fullBackupImport, isTrue);
    });

    test('explicit null capabilities cannot permit full appliance backup',
        () async {
      state({'version': '0.6.632', 'nodes': [], 'capabilities': null});
      await expectLater(api.getDeploymentCapabilities(), throwsStateError);
    });

    test('valid older capabilities preserve the appliance contract', () async {
      state({
        'capabilities': {
          'features': ['future-feature'],
          'hubs': []
        }
      });
      final deployment = await api.getDeploymentCapabilities();
      expect(deployment.fullBackupExport, isTrue);
    });

    test('explicit deployment keeps whole backup disabled', () async {
      state({
        'capabilities': {
          'deployment': {'kind': 'home_assistant_addon'}
        }
      });
      final deployment = await api.getDeploymentCapabilities();
      expect(deployment.fullBackupExport, isFalse);
      expect(deployment.fullBackupImport, isFalse);
    });
  });

  group('putConfigurationBundle', () {
    test('sends profile bundles to the server profile-bundle route', () async {
      when(
        () => dio.put(
          any(),
          data: any(named: 'data'),
          options: any(named: 'options'),
        ),
      ).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/profile-bundle'),
          statusCode: 200,
          data: {'kind': 'profile_bundle'},
        ),
      );

      final result = await api.putConfigurationBundle(
        const {'kind': 'profile_bundle'},
      );

      expect(result, {'kind': 'profile_bundle'});
      final captured = verify(
        () => dio.put(
          captureAny(),
          data: captureAny(named: 'data'),
          options: captureAny(named: 'options'),
        ),
      ).captured;
      expect(captured[0], 'api/profile-bundle');
      expect(captured[1], {'kind': 'profile_bundle'});
      expect((captured[2] as Options).validateStatus?.call(500), isTrue);
    });
  });

  group('factory default and share bundles', () {
    test('uses profile-bundle aliases for factory default and reset', () async {
      when(
        () => dio.get(
          any(),
          options: any(named: 'options'),
        ),
      ).thenAnswer(
        (_) async => Response(
          requestOptions:
              RequestOptions(path: 'api/profile-bundle/factory-default'),
          statusCode: 200,
          data: {'kind': 'profile_bundle', 'factory': true},
        ),
      );
      when(
        () => dio.post(
          any(),
          options: any(named: 'options'),
        ),
      ).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/profile-bundle/reset'),
          statusCode: 200,
          data: {'kind': 'profile_bundle', 'reset': true},
        ),
      );

      final factoryDefault = await api.getFactoryDefaultConfigurationBundle();
      final reset = await api.resetConfigurationBundle();

      expect(factoryDefault, {'kind': 'profile_bundle', 'factory': true});
      expect(reset, {'kind': 'profile_bundle', 'reset': true});
      verify(
        () => dio.get(
          'api/profile-bundle/factory-default',
          options: any(named: 'options'),
        ),
      ).called(1);
      verify(
        () => dio.post(
          'api/profile-bundle/reset',
          options: any(named: 'options'),
        ),
      ).called(1);
    });

    test('uses share-bundle aliases for portable sharing', () async {
      when(
        () => dio.get(
          any(),
          options: any(named: 'options'),
        ),
      ).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/share-bundle'),
          statusCode: 200,
          data: {'kind': 'profile_bundle'},
        ),
      );
      when(
        () => dio.put(
          any(),
          data: any(named: 'data'),
          options: any(named: 'options'),
        ),
      ).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/share-bundle'),
          statusCode: 200,
          data: {'kind': 'profile_bundle', 'saved': true},
        ),
      );
      when(
        () => dio.post(
          any(),
          options: any(named: 'options'),
        ),
      ).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'api/share-bundle/reset'),
          statusCode: 200,
          data: {'kind': 'profile_bundle', 'reset': true},
        ),
      );

      final current = await api.getShareBundle();
      final factoryDefault = await api.getFactoryDefaultShareBundle();
      final saved = await api.putShareBundle({'kind': 'profile_bundle'});
      final reset = await api.resetShareBundle();

      expect(current, {'kind': 'profile_bundle'});
      expect(factoryDefault, {'kind': 'profile_bundle'});
      expect(saved, {'kind': 'profile_bundle', 'saved': true});
      expect(reset, {'kind': 'profile_bundle', 'reset': true});
      verify(
        () => dio.get(
          'api/share-bundle',
          options: any(named: 'options'),
        ),
      ).called(1);
      verify(
        () => dio.get(
          'api/share-bundle/factory-default',
          options: any(named: 'options'),
        ),
      ).called(1);
      verify(
        () => dio.put(
          'api/share-bundle',
          data: {'kind': 'profile_bundle'},
          options: any(named: 'options'),
        ),
      ).called(1);
      verify(
        () => dio.post(
          'api/share-bundle/reset',
          options: any(named: 'options'),
        ),
      ).called(1);
    });
  });

  group('fetchBackupJson', () {
    test('requests plain text with include_secrets when requested', () async {
      late http.Request request;
      final client = MockClient((incoming) async {
        request = incoming;
        return http.Response.bytes(
          '{"kind":"backup_bundle"}'.codeUnits,
          200,
          headers: const {'content-type': 'application/json'},
        );
      });
      api = RhythmBundleApi(
        baseUrl: 'http://test/',
        dio: dio,
        httpClient: client,
      );

      final result = await api.fetchBackupJson(includeSecrets: true);

      expect(result, '{"kind":"backup_bundle"}');
      expect(request.method, 'GET');
      expect(request.url.toString(),
          'http://test/api/backup?include_secrets=true');
      expect(request.headers['accept'], 'application/json');
    });

    test('throws RhythmApiException with HTTP status and body on failure',
        () async {
      api = RhythmBundleApi(
        baseUrl: 'http://test/',
        dio: dio,
        httpClient: MockClient((_) async {
          return http.Response.bytes(
            'backup route failed'.codeUnits,
            500,
          );
        }),
      );

      expect(
        () => api.fetchBackupJson(includeSecrets: true),
        throwsA(
          isA<RhythmApiException>()
              .having((e) => e.statusCode, 'statusCode', 500)
              .having(
                (e) => e.serverMessage,
                'serverMessage',
                'backup route failed',
              ),
        ),
      );
    });

    test('wraps connection failures with a readable server message', () async {
      api = RhythmBundleApi(
        baseUrl: 'http://test/',
        dio: dio,
        httpClient: MockClient((_) async {
          throw http.ClientException('Could not reach the server.');
        }),
      );

      expect(
        () => api.fetchBackupJson(includeSecrets: true),
        throwsA(
          isA<RhythmApiException>()
              .having((e) => e.statusCode, 'statusCode', isNull)
              .having(
                (e) => e.serverMessage,
                'serverMessage',
                'Could not reach the server.',
              ),
        ),
      );
    });
  });

  group('restoreBackupJson', () {
    test('sends raw json and parses plain-text json responses', () async {
      late http.Request request;
      api = RhythmBundleApi(
        baseUrl: 'http://test/',
        dio: dio,
        httpClient: MockClient((incoming) async {
          request = incoming;
          return http.Response.bytes(
            '{"kind":"backup_bundle","redacted":true}'.codeUnits,
            200,
            headers: const {'content-type': 'application/json'},
          );
        }),
      );

      final result = await api.restoreBackupJson('{"kind":"backup_bundle"}');

      expect(result, {'kind': 'backup_bundle', 'redacted': true});
      expect(request.method, 'PUT');
      expect(request.url.toString(), 'http://test/api/backup');
      expect(request.body, '{"kind":"backup_bundle"}');
      expect(request.headers['content-type'], 'application/json');
    });
  });
}
