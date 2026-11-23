import 'dart:convert';
import 'dart:io';

import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  test(
    'negotiates and calls the selected hub with its scoped bearer and validates correlated receipts',
    () async {
      final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      addTearDown(() => server.close(force: true));
      final api = RhythmMcpApi(
        baseUrl: 'http://127.0.0.1:${server.port}',
        agentToken: 'fixture-agent',
      );
      addTearDown(api.close);
      final methods = <String>[];
      server.listen((request) async {
        expect(request.uri.path, '/api/assistant/mcp');
        expect(request.headers.value('authorization'), 'Bearer fixture-agent');
        final msg = jsonDecode(await utf8.decoder.bind(request).join()) as Map;
        final method = msg['method'] as String;
        methods.add(method);
        request.response.headers.contentType = ContentType.json;
        if (method == 'notifications/initialized') {
          expect(msg.containsKey('id'), isFalse);
          request.response.statusCode = 202;
        } else {
          final result = switch (method) {
            'initialize' => {'protocolVersion': '2025-06-18'},
            'tools/list' => {
              'tools': [
                {'name': 'explain_room_output'},
              ],
            },
            _ => {
              'content': [
                {'type': 'text', 'text': '{"canonical_brightness":42}'},
              ],
            },
          };
          request.response.write(
            jsonEncode({
              'jsonrpc': '2.0',
              'id': methods.length > 4 ? 0 : msg['id'],
              'result': result,
            }),
          );
        }
        await request.response.close();
      });
      expect(await api.discover(), {'explain_room_output'});
      expect(await api.call('explain_room_output', {'room_id': 'room'}), {
        'canonical_brightness': 42,
      });
      await expectLater(
        api.call('explain_room_output', {'room_id': 'room'}),
        throwsFormatException,
      );
      expect(methods.take(4), [
        'initialize',
        'notifications/initialized',
        'tools/list',
        'tools/call',
      ]);
    },
  );
}
