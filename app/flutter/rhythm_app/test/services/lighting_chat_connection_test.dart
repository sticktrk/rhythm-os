import 'dart:convert';
import 'dart:io';

import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/lighting_chat_service.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  final overrides = HttpOverrides.current;
  setUpAll(() => HttpOverrides.global = null);
  tearDownAll(() => HttpOverrides.global = overrides);

  test(
      'local first use issues once, saved access travels, and only an explicit local reconnect replaces revoked access',
      () async {
    FlutterSecureStorage.setMockInitialValues({});
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    addTearDown(() => server.close(force: true));
    var issued = 0;
    final revoked = <String>{};
    server.listen((req) async {
      final bearer = req.headers.value('authorization');
      final payload = jsonDecode(await utf8.decoder.bind(req).join());
      final res = req.response;
      res.headers.contentType = ContentType.json;
      if (req.uri.path == '/api/auth/agent-tokens') {
        expect(bearer, 'Bearer fixture-owner');
        expect(payload, {'unattended_changes': false});
        ++issued;
        res.write(
            jsonEncode({'token_id': 'id-$issued', 'token': 'fixture-$issued'}));
      } else {
        expect(req.uri.path, '/api/assistant/mcp');
        if (revoked.contains(bearer)) {
          res.statusCode = 401;
        } else if (payload['method'] == 'notifications/initialized') {
          res.statusCode = 202;
        } else {
          final result = switch (payload['method']) {
            'initialize' => {'protocolVersion': '2025-06-18'},
            'tools/list' => {
                'tools': [
                  {'name': 'explain_room_output'}
                ]
              },
            _ => {
                'content': [
                  {
                    'type': 'text',
                    'text': '{"server_instance_id":"instance","nodes":[]}'
                  }
                ]
              },
          };
          res.write(jsonEncode(
              {'id': payload['id'], 'jsonrpc': '2.0', 'result': result}));
        }
      }
      await res.close();
    });
    LightingChatConnection connection(
            {bool local = true,
            String? owner = 'fixture-owner',
            String account = 'resident'}) =>
        LightingChatConnection(
            baseUrl: 'http://127.0.0.1:${server.port}',
            ownerToken: owner,
            local: local,
            accountId: account,
            instance: 'instance',
            current: () => true);
    await expectLater(connection(local: false).open(),
        throwsA(LightingChatAccessIssue.localNetwork));
    await expectLater(
        connection(owner: null).open(), throwsA(LightingChatAccessIssue.owner));
    expect(issued, 0);
    final clients =
        await Future.wait([connection().open(), connection().open()]);
    for (final client in clients) {
      client.close();
    }
    expect(issued, 1);
    (await connection(local: false, owner: null).open()).close();
    await expectLater(connection(local: false, account: 'different').open(),
        throwsA(LightingChatAccessIssue.localNetwork));
    revoked.add('Bearer fixture-1');
    await expectLater(
        connection().open(), throwsA(LightingChatAccessIssue.revoked));
    await expectLater(
        connection().open(), throwsA(LightingChatAccessIssue.revoked));
    await expectLater(connection(local: false).open(reconnect: true),
        throwsA(LightingChatAccessIssue.revoked));
    expect(issued, 1);
    (await connection().open(reconnect: true)).close();
    (await connection().open()).close();
    expect(issued, 2);
  });
}
