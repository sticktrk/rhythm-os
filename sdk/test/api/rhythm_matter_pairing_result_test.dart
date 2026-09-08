import 'dart:convert';
import 'dart:io';

import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  test(
    'reconciles only valid receipts for the requested Matter attempt',
    () async {
      final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      var httpStatus = 200;
      Object body = {};
      final paths = <String>[];
      server.listen((request) async {
        paths.add(request.uri.path);
        expect(request.headers.value('authorization'), 'Bearer test-token');
        request.response
          ..statusCode = httpStatus
          ..headers.contentType = ContentType.json
          ..write(jsonEncode(body));
        await request.response.close();
      });
      final api = RhythmMatterApi(
        baseUrl: 'http://127.0.0.1:${server.port}',
        authToken: 'test-token',
      );
      try {
        body = {
          'session_id': 'attempt-1',
          'state': 'pending',
          'hub_type': 'matter',
        };
        expect((await api.getPairingResult('attempt-1'))?.status, 'pending');
        httpStatus = 404;
        body = {'session_id': 'attempt-1', 'state': 'not_found'};
        expect((await api.getPairingResult('attempt-1'))?.status, 'not_found');
        httpStatus = 200;
        for (final status in ['failed', 'complete']) {
          body = {
            'session_id': 'attempt-1',
            'state': 'terminal',
            'hub_type': 'matter',
            'result': {
              'hub_type': 'matter',
              'status': status,
              if (status == 'complete')
                'device': {
                  'device_id': 'matter-42',
                  'name': 'Test bulb',
                  'device_type': 'light',
                },
              if (status == 'failed') 'error': 'Handoff failed',
              if (status == 'complete')
                'warnings': ['Recovery code was not saved.'],
            },
          };
          final receipt = await api.getPairingResult('attempt-1');
          expect(receipt?.status, status);
          expect(
              receipt?.warnings,
              status == 'complete'
                  ? ['Recovery code was not saved.']
                  : isEmpty);
          if (status == 'complete') {
            expect(receipt?.device?['device_id'], 'matter-42');
          }
        }
        for (final invalid in [
          {
            'session_id': 'other-attempt',
            'state': 'pending',
            'hub_type': 'matter',
          },
          {'session_id': 'attempt-1', 'state': 'pending', 'hub_type': 'hue'},
          {
            'session_id': 'attempt-1',
            'state': 'terminal',
            'hub_type': 'matter',
          },
          {'session_id': 'attempt-1', 'state': 'not_found'},
          {'message': 'Not found'},
        ]) {
          body = invalid;
          expect(await api.getPairingResult('attempt-1'), isNull);
        }
        httpStatus = 503;
        body = {'session_id': 'attempt-1', 'state': 'not_found'};
        expect(await api.getPairingResult('attempt-1'), isNull);
        expect(paths.toSet(), {'/api/devices/pair/attempt-1'});
      } finally {
        await server.close(force: true);
      }
    },
  );
}
