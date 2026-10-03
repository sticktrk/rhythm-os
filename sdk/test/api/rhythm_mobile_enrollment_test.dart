import 'dart:convert';
import 'package:dio/dio.dart';
import 'package:mocktail/mocktail.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

import '../helpers/mock_dio.dart';

void main() {
  test('auth status explicitly advertises enrollment and preserves old servers', () {
    expect(RhythmAuthStatus.fromJson({}).mobileEnrollmentAvailable, isFalse);
    expect(RhythmAuthStatus.fromJson({'mobile_enrollment_available': true}).mobileEnrollmentAvailable, isTrue);
  });
  Map<String, dynamic> payload() => {
        'format': 'rhythm-mobile-enrollment',
        'version': 1,
        'code': 'single-use-approval',
        'server_instance_id': 'installation-a',
        'expires_at_epoch_ms': DateTime.now()
            .add(const Duration(minutes: 5))
            .millisecondsSinceEpoch,
      };

  test('parses versioned approval and rejects incomplete or old payloads', () {
    final enrollment = RhythmMobileEnrollment.parse(jsonEncode(payload()));
    expect(enrollment.serverInstanceId, 'installation-a');
    expect(enrollment.isExpired(), isFalse);
    expect(
        enrollment.isExpired(
            now: DateTime.fromMillisecondsSinceEpoch(
                enrollment.expiresAtEpochMs)),
        isTrue);
    for (final value in [
      'secret',
      '{}',
      jsonEncode({...payload(), 'version': 2})
    ]) {
      expect(() => RhythmMobileEnrollment.parse(value), throwsFormatException);
    }
  });

  test('exchange is instance bound and never places approval in URL', () async {
    final dio = MockDio();
    final api = RhythmAuthApi(baseUrl: 'http://test/', dio: dio);
    when(() => dio.post<Map<String, dynamic>>(any(), data: any(named: 'data')))
        .thenAnswer((_) async => Response(
              requestOptions: RequestOptions(),
              statusCode: 200,
              data: {
                'token': 'phone-token',
                'token_id': 'phone-id',
                'server_instance_id': 'installation-a'
              },
            ));
    final enrollment = RhythmMobileEnrollment.parse(jsonEncode(payload()));
    final claim = await api.exchangeMobileEnrollment(enrollment);
    expect(claim.token, 'phone-token');
    verify(() =>
        dio.post<Map<String, dynamic>>('api/addon/enrollment/exchange', data: {
          'code': 'single-use-approval',
          'server_instance_id': 'installation-a',
          'label': 'Rhythm app',
        })).called(1);
  });

  test('mismatched installation response does not install a credential',
      () async {
    final dio = MockDio();
    final api = RhythmAuthApi(baseUrl: 'http://test/', dio: dio);
    when(() => dio.post<Map<String, dynamic>>(any(), data: any(named: 'data')))
        .thenAnswer((_) async => Response(
              requestOptions: RequestOptions(),
              statusCode: 200,
              data: {
                'token': 'wrong-token',
                'token_id': 'phone-id',
                'server_instance_id': 'installation-b'
              },
            ));
    await expectLater(
        api.exchangeMobileEnrollment(
            RhythmMobileEnrollment.parse(jsonEncode(payload()))),
        throwsStateError);
  });

  test('expired approval never reaches the network', () async {
    final dio = MockDio();
    final api = RhythmAuthApi(baseUrl: 'http://test/', dio: dio);
    await expectLater(
        api.exchangeMobileEnrollment(const RhythmMobileEnrollment(
          code: 'expired',
          serverInstanceId: 'installation-a',
          expiresAtEpochMs: 1,
        )),
        throwsFormatException);
    verifyNever(
        () => dio.post<Map<String, dynamic>>(any(), data: any(named: 'data')));
  });
}
