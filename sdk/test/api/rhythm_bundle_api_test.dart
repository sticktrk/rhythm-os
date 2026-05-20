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
