import 'package:dio/dio.dart';
import 'package:mocktail/mocktail.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';
import '../helpers/mock_dio.dart';

void main() {
  late MockDio dio;
  late RhythmServerApi api;
  setUp(() {
    registerFallbackValue(Options());
    dio = MockDio();
    api = RhythmServerApi(dio);
  });
  test(
      'revision-checked secret requests are uncached and keep omitted passwords',
      () async {
    Map? body;
    when(() => dio.request(any(),
        data: any(named: 'data'),
        options: any(named: 'options'))).thenAnswer((call) async {
      expect(
          (call.namedArguments[#options] as Options).headers?['Cache-Control'],
          'no-store');
      body = call.namedArguments[#data] as Map?;
      return Response(
          requestOptions: RequestOptions(path: 'wifi'),
          statusCode: 200,
          data: {
            'revision': 7,
            'default_id': 'a',
            'profiles': [
              {'id': 'a', 'ssid': 'Fixture'}
            ]
          });
    });
    final catalog = await api.getWifiProfiles();
    expect(catalog.defaultId, 'a');
    await api.updateWifiProfile(
        revision: 7,
        action: 'save',
        correlationId: 'journey',
        id: 'a',
        ssid: 'Edited');
    expect(body, {
      'revision': 7,
      'action': 'save',
      'correlation_id': 'journey',
      'id': 'a',
      'ssid': 'Edited'
    });
    expect(catalog.profiles.first.toString(), isNot(contains('Fixture')));
  });
  test('server errors never expose secrets', () async {
    for (final status in [401, 403, 409, 500]) {
      when(() => dio.request(any(),
              data: any(named: 'data'), options: any(named: 'options')))
          .thenAnswer((_) async => Response(
              requestOptions: RequestOptions(path: 'wifi'),
              statusCode: status,
              data: {'error': 'fixture-secret'}));
      try {
        await api.getWifiProfiles();
        fail('expected failure');
      } catch (e) {
        expect(e, isA<RhythmWifiException>());
        expect(e.toString(), isNot(contains('fixture-secret')));
      }
    }
  });
  test('pending and uncertain receipts cannot claim success', () async {
    for (final status in ['pending', 'unknown', 'complete']) {
      when(() => dio.request(any(),
          data: any(named: 'data'),
          options: any(named: 'options'))).thenAnswer((_) async => Response(
              requestOptions: RequestOptions(path: 'wifi'),
              statusCode: 200,
              data: {
                'operation_id': 'op',
                'status': status,
                'outcome': {'code': 'recovery_required'}
              }));
      final receipt = await api.getMatterWifiChange('op');
      expect(receipt.succeeded, isFalse);
      expect(receipt.isPending, status != 'complete');
    }
    expect(
        const RhythmCapabilities()
            .supportsFeature(RhythmFeature.savedWifiProfiles),
        isFalse);
    expect(
        const RhythmCapabilities()
            .supportsFeature(RhythmFeature.matterWifiChange),
        isFalse);
  });
  test('start carries one operation ID and profile reference only', () async {
    Map? body;
    when(() => dio.request(any(),
        data: any(named: 'data'),
        options: any(named: 'options'))).thenAnswer((call) async {
      body = call.namedArguments[#data] as Map;
      return Response(
          requestOptions: RequestOptions(path: 'wifi'),
          statusCode: 200,
          data: {'operation_id': 'op', 'status': 'pending'});
    });
    expect(
        (await api.startMatterWifiChange(
                operationId: 'op', deviceId: 'matter-1', profileId: 'profile'))
            .isPending,
        isTrue);
    expect(body, {
      'operation_id': 'op',
      'device_id': 'matter-1',
      'profile_id': 'profile'
    });
  });
}
