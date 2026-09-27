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
      when(
        () => dio.request(
          any(),
          data: any(named: 'data'),
          options: any(named: 'options'),
        ),
      ).thenAnswer((call) async {
        expect(
          (call.namedArguments[#options] as Options).headers?['Cache-Control'],
          'no-store',
        );
        body = call.namedArguments[#data] as Map?;
        return Response(
          requestOptions: RequestOptions(path: 'wifi'),
          statusCode: 200,
          data: {
            'revision': 7,
            'default_id': 'a',
            'box_profile_id': 'b',
            'profiles': [
              {'id': 'a', 'ssid': 'Fixture'},
            ],
          },
        );
      });
      final catalog = await api.getWifiProfiles();
      expect(catalog.defaultId, 'a');
      expect(catalog.boxProfileId, 'b');
      await api.updateWifiProfile(
        revision: 7,
        action: 'save',
        correlationId: 'journey',
        id: 'a',
        ssid: 'Edited',
      );
      expect(body, {
        'revision': 7,
        'action': 'save',
        'correlation_id': 'journey',
        'id': 'a',
        'ssid': 'Edited',
      });
      expect(catalog.profiles.first.toString(), isNot(contains('Fixture')));
    },
  );
  test(
    'current Wi-Fi is an uncached observation with a previous-server fallback',
    () async {
      var status = 200;
      Object data = {
        'status': 'connected',
        'ssid': 'Observed fixture',
        'observed_at_ms': 42,
      };
      when(
        () => dio.request(
          any(),
          data: any(named: 'data'),
          options: any(named: 'options'),
        ),
      ).thenAnswer((call) async {
        expect(call.positionalArguments[0], 'api/matter/wifi-network/matter-1');
        expect(
          (call.namedArguments[#options] as Options).headers?['Cache-Control'],
          'no-store',
        );
        return Response(
          requestOptions: RequestOptions(path: 'network'),
          statusCode: status,
          data: data,
        );
      });
      final network = await api.getMatterWifiNetwork('matter-1');
      expect(network.ssid, 'Observed fixture');
      expect(network.observedAtMs, 42);
      expect(network.toString(), isNot(contains('Observed fixture')));
      data = {'status': 'offline', 'ssid': 'stale fixture'};
      expect((await api.getMatterWifiNetwork('matter-1')).ssid, isNull);
      data = {'status': 'new_future_state', 'ssid': 'stale fixture'};
      expect(
        (await api.getMatterWifiNetwork('matter-1')).status,
        'unavailable',
      );
      status = 404;
      expect(
        (await api.getMatterWifiNetwork('matter-1')).status,
        'unsupported_server',
      );
      status = 403;
      await expectLater(
        api.getMatterWifiNetwork('matter-1'),
        throwsA(isA<RhythmWifiException>()),
      );
    },
  );
  test('network conflict exposes only bounded recovery metadata', () async {
    when(
      () => dio.request(
        any(),
        data: any(named: 'data'),
        options: any(named: 'options'),
      ),
    ).thenAnswer(
      (_) async => Response(
        requestOptions: RequestOptions(path: 'network'),
        statusCode: 409,
        data: {
          'reason': 'device_recovering',
          'retry_after_ms': 120000,
          'error': 'secret fixture',
        },
      ),
    );
    try {
      await api.startMatterWifiChange(
        operationId: 'op',
        deviceId: 'matter-1',
        profileId: 'p',
      );
      fail('expected conflict');
    } on RhythmWifiException catch (error) {
      expect(error.reason, 'device_recovering');
      expect(error.retryAfterMs, 120000);
      expect(error.toString(), isNot(contains('secret')));
    }
  });
  test('server errors never expose secrets', () async {
    for (final status in [401, 403, 409, 500]) {
      when(
        () => dio.request(
          any(),
          data: any(named: 'data'),
          options: any(named: 'options'),
        ),
      ).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'wifi'),
          statusCode: status,
          data: {'error': 'fixture-secret'},
        ),
      );
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
      when(
        () => dio.request(
          any(),
          data: any(named: 'data'),
          options: any(named: 'options'),
        ),
      ).thenAnswer(
        (_) async => Response(
          requestOptions: RequestOptions(path: 'wifi'),
          statusCode: 200,
          data: {
            'operation_id': 'op',
            'status': status,
            'outcome': {'code': 'recovery_required'},
          },
        ),
      );
      final receipt = await api.getMatterWifiChange('op');
      expect(receipt.succeeded, isFalse);
      expect(receipt.isPending, status != 'complete');
    }
    expect(
      const RhythmCapabilities().supportsFeature(
        RhythmFeature.savedWifiProfiles,
      ),
      isFalse,
    );
    expect(
      const RhythmCapabilities().supportsFeature(
        RhythmFeature.matterWifiChange,
      ),
      isFalse,
    );
  });
  test('start carries one operation ID and profile reference only', () async {
    Map? body;
    when(
      () => dio.request(
        any(),
        data: any(named: 'data'),
        options: any(named: 'options'),
      ),
    ).thenAnswer((call) async {
      body = call.namedArguments[#data] as Map;
      return Response(
        requestOptions: RequestOptions(path: 'wifi'),
        statusCode: 200,
        data: {'operation_id': 'op', 'status': 'pending'},
      );
    });
    expect(
      (await api.startMatterWifiChange(
        operationId: 'op',
        deviceId: 'matter-1',
        profileId: 'profile',
      )).isPending,
      isTrue,
    );
    expect(body, {
      'operation_id': 'op',
      'device_id': 'matter-1',
      'profile_id': 'profile',
    });
  });
  test('a Box that cannot check a network never blocks saving it', () async {
    Response<dynamic> reply(int status, [Object? data]) => Response(
      requestOptions: RequestOptions(path: 'verify'),
      statusCode: status,
      data: data,
    );
    var status = 404;
    Object? data;
    when(
      () => dio.post(
        any(),
        data: any(named: 'data'),
        options: any(named: 'options'),
      ),
    ).thenAnswer((_) async => reply(status, data));
    when(
      () => dio.get(any(), options: any(named: 'options')),
    ).thenAnswer((_) async => reply(status, data));
    Future<RhythmWifiCheck> start() =>
        api.startWifiCheck(operationId: 'op', ssid: 'Fixture', password: 'pw');

    // Older firmware, a busy radio, or a host without its own Wi-Fi.
    expect((await start()).state, RhythmWifiCheckState.unavailable);
    status = 409;
    expect((await start()).state, RhythmWifiCheckState.unavailable);
    status = 200;
    data = <String, dynamic>{'state': 'running', 'reason': null};
    expect((await start()).state, RhythmWifiCheckState.running);
    data = <String, dynamic>{'state': 'failed', 'reason': 'join_failed'};
    final failed = await api.getWifiCheck('op');
    expect(failed?.state, RhythmWifiCheckState.failed);
    expect(failed?.reason, 'join_failed');
    // The Box is offline while it checks: not an answer.
    when(
      () => dio.get(any(), options: any(named: 'options')),
    ).thenThrow(DioException(requestOptions: RequestOptions(path: 'verify')));
    expect(await api.getWifiCheck('op'), isNull);
  });
}
