import 'package:dio/dio.dart';
import 'package:mocktail/mocktail.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

import '../helpers/mock_dio.dart';

void main() {
  late MockDio dio;
  late RhythmRemoteAccessApi api;

  setUp(() {
    dio = MockDio();
    api = RhythmRemoteAccessApi(
      baseUrl: 'http://test',
      dio: dio,
      authToken: 'owner-token',
    );
  });

  test('puts config and parses redacted status', () async {
    when(() => dio.put<Map<String, dynamic>>(
          any(),
          data: any(named: 'data'),
        )).thenAnswer(
      (_) async => Response<Map<String, dynamic>>(
        requestOptions: RequestOptions(path: 'api/remote-access/config'),
        statusCode: 200,
        data: {
          'status': 'ok',
          'enabled': true,
          'configured': true,
          'hostname': 'hub.devices.rhythm.lighting',
          'tunnel_id': 'tunnel-id',
          'tunnel_name': 'tunnel-name',
          'updated_at_epoch_ms': 123,
          'cloudflared_available': true,
          'cloudflared_version': 'cloudflared version 2026.5.2',
          'service_available': true,
          'service_running': false,
          'supervisor_state': 'running',
          'supervisor_pid': 11,
          'child_pid': 12,
          'restart_count': 2,
          'last_started_epoch_secs': 1234,
          'last_exit_epoch_secs': 1200,
          'last_exit_code': 1,
          'next_restart_epoch_secs': 1260,
          'metrics_addr': '127.0.0.1:54449',
          'metrics_available': true,
          'connector_healthy': true,
          'registered_connections': 1,
        },
      ),
    );

    final result = await api.putConfig(
      hostname: 'hub.devices.rhythm.lighting',
      connectorToken: 'connector-secret',
      tunnelId: 'tunnel-id',
      tunnelName: 'tunnel-name',
    );

    expect(result.enabled, isTrue);
    expect(result.configured, isTrue);
    expect(result.hostname, 'hub.devices.rhythm.lighting');
    expect(result.tunnelId, 'tunnel-id');
    expect(result.serviceRunning, isFalse);
    expect(result.cloudflaredVersion, 'cloudflared version 2026.5.2');
    expect(result.supervisorState, 'running');
    expect(result.supervisorPid, 11);
    expect(result.childPid, 12);
    expect(result.restartCount, 2);
    expect(result.lastStartedEpochSecs, 1234);
    expect(result.lastExitEpochSecs, 1200);
    expect(result.lastExitCode, 1);
    expect(result.nextRestartEpochSecs, 1260);
    expect(result.metricsAddr, '127.0.0.1:54449');
    expect(result.metricsAvailable, isTrue);
    expect(result.connectorHealthy, isTrue);
    expect(result.registeredConnections, 1);

    final captured = verify(() => dio.put<Map<String, dynamic>>(
          'api/remote-access/config',
          data: captureAny(named: 'data'),
        )).captured.single as Map<String, dynamic>;
    expect(captured, {
      'enabled': true,
      'hostname': 'hub.devices.rhythm.lighting',
      'connector_token': 'connector-secret',
      'tunnel_id': 'tunnel-id',
      'tunnel_name': 'tunnel-name',
    });
  });

  test('deletes config and parses disabled status', () async {
    when(() => dio.delete<Map<String, dynamic>>(any())).thenAnswer(
      (_) async => Response<Map<String, dynamic>>(
        requestOptions: RequestOptions(path: 'api/remote-access/config'),
        statusCode: 200,
        data: {
          'status': 'ok',
          'enabled': false,
          'configured': false,
          'updated_at_epoch_ms': 0,
          'cloudflared_available': false,
          'service_available': true,
          'service_running': false,
        },
      ),
    );

    final result = await api.clearConfig();

    expect(result.enabled, isFalse);
    expect(result.configured, isFalse);
    verify(() => dio.delete<Map<String, dynamic>>('api/remote-access/config'))
        .called(1);
  });
}
