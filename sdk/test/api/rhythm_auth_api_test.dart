import 'package:dio/dio.dart';
import 'package:mocktail/mocktail.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

import '../helpers/mock_dio.dart';

void main() {
  late MockDio dio;
  late RhythmAuthApi api;

  setUp(() {
    dio = MockDio();
    api = RhythmAuthApi(
      baseUrl: 'http://test',
      dio: dio,
      authToken: 'owner-token',
    );
  });

  group('issueSupportToken', () {
    test('posts support token request and parses issued token', () async {
      when(() => dio.post<Map<String, dynamic>>(
            any(),
            data: any(named: 'data'),
          )).thenAnswer(
        (_) async => Response<Map<String, dynamic>>(
          requestOptions: RequestOptions(path: 'api/auth/support-token'),
          statusCode: 200,
          data: {
            'status': 'ok',
            'token_id': 'support-1',
            'token': 'rhythm_support_raw',
            'role': 'support',
          },
        ),
      );

      final result = await api.issueSupportToken(label: ' admin support ');

      expect(result.tokenId, 'support-1');
      expect(result.token, 'rhythm_support_raw');
      expect(result.role, 'support');

      final captured = verify(() => dio.post<Map<String, dynamic>>(
            'api/auth/support-token',
            data: captureAny(named: 'data'),
          )).captured.single as Map<String, dynamic>;
      expect(captured, {'label': 'admin support'});
    });

    test('omits blank support token labels', () async {
      when(() => dio.post<Map<String, dynamic>>(
            any(),
            data: any(named: 'data'),
          )).thenAnswer(
        (_) async => Response<Map<String, dynamic>>(
          requestOptions: RequestOptions(path: 'api/auth/support-token'),
          statusCode: 200,
          data: {
            'status': 'ok',
            'token_id': 'support-1',
            'token': 'rhythm_support_raw',
            'role': 'support',
          },
        ),
      );

      await api.issueSupportToken(label: '  ');

      final captured = verify(() => dio.post<Map<String, dynamic>>(
            'api/auth/support-token',
            data: captureAny(named: 'data'),
          )).captured.single as Map<String, dynamic>;
      expect(captured, isEmpty);
    });
  });

  group('createCloudJoinProof', () {
    test('posts join proof request and parses proof payload', () async {
      when(() => dio.post<Map<String, dynamic>>(any())).thenAnswer(
        (_) async => Response<Map<String, dynamic>>(
          requestOptions: RequestOptions(path: 'api/cloud/join-proof'),
          statusCode: 200,
          data: {
            'status': 'ok',
            'proof_version': 'activity-token-hmac-v1',
            'algorithm': 'hmac-sha256',
            'server_instance_id': 'srv-kitchen',
            'home_id': 'home-1',
            'hub_id': 'hub-1',
            'token_id': 'token-1',
            'issued_at_epoch_ms': 1000,
            'expires_at_epoch_ms': 121000,
            'nonce': '0123456789abcdef',
            'signature':
                '41b380e3ff97e37dd008c706e58ff422adbab4bcc0ad822dc98480319a585dc9',
          },
        ),
      );

      final proof = await api.createCloudJoinProof();

      expect(proof.serverInstanceId, 'srv-kitchen');
      expect(proof.homeId, 'home-1');
      expect(proof.hubId, 'hub-1');
      expect(proof.tokenId, 'token-1');
      expect(proof.toJson(),
          containsPair('proof_version', 'activity-token-hmac-v1'));

      verify(() => dio.post<Map<String, dynamic>>('api/cloud/join-proof'))
          .called(1);
    });
  });

  group('setSettings', () {
    test('puts auth settings and parses issued owner token', () async {
      when(() => dio.put<Map<String, dynamic>>(
            any(),
            data: any(named: 'data'),
          )).thenAnswer(
        (_) async => Response<Map<String, dynamic>>(
          requestOptions: RequestOptions(path: 'api/auth/settings'),
          statusCode: 200,
          data: {
            'status': 'ok',
            'requires_auth': true,
            'owner_configured': true,
            'token_count': 1,
            'claim_available': false,
            'token_id': 'token-1',
            'token': 'rhythm_owner_raw',
          },
        ),
      );

      final result = await api.setSettings(
        requireApiAuth: true,
        label: ' phone ',
      );

      expect(result.requiresAuth, isTrue);
      expect(result.ownerConfigured, isTrue);
      expect(result.tokenCount, 1);
      expect(result.claimAvailable, isFalse);
      expect(result.tokenId, 'token-1');
      expect(result.token, 'rhythm_owner_raw');

      final captured = verify(() => dio.put<Map<String, dynamic>>(
            'api/auth/settings',
            data: captureAny(named: 'data'),
          )).captured.single as Map<String, dynamic>;
      expect(captured, {
        'require_api_auth': true,
        'label': 'phone',
      });
    });

    test('omits blank labels', () async {
      when(() => dio.put<Map<String, dynamic>>(
            any(),
            data: any(named: 'data'),
          )).thenAnswer(
        (_) async => Response<Map<String, dynamic>>(
          requestOptions: RequestOptions(path: 'api/auth/settings'),
          statusCode: 200,
          data: {
            'status': 'ok',
            'requires_auth': false,
            'owner_configured': true,
            'token_count': 1,
            'claim_available': false,
          },
        ),
      );

      final result = await api.setSettings(
        requireApiAuth: false,
        label: '  ',
      );

      expect(result.requiresAuth, isFalse);
      expect(result.token, isNull);

      final captured = verify(() => dio.put<Map<String, dynamic>>(
            'api/auth/settings',
            data: captureAny(named: 'data'),
          )).captured.single as Map<String, dynamic>;
      expect(captured, {'require_api_auth': false});
    });

    test('throws RhythmApiException on DioException', () async {
      when(() => dio.put<Map<String, dynamic>>(
            any(),
            data: any(named: 'data'),
          )).thenThrow(
        DioException(
          requestOptions: RequestOptions(path: 'api/auth/settings'),
          response: Response(
            requestOptions: RequestOptions(path: 'api/auth/settings'),
            statusCode: 401,
          ),
          type: DioExceptionType.badResponse,
        ),
      );

      expect(
        () => api.setSettings(requireApiAuth: false),
        throwsA(
          isA<RhythmApiException>().having(
            (e) => e.statusCode,
            'statusCode',
            401,
          ),
        ),
      );
    });
  });
}
