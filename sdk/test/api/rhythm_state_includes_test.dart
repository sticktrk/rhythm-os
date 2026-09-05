import 'dart:convert';
import 'dart:io';
import 'package:dio/dio.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  test('one authoritative selective hello; old servers fall back to full state',
      () async {
    for (final legacy in [false, true]) {
      final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      final queries = <Map<String, String>>[];
      final auth = <String?>[];
      server.listen((request) async {
        queries.add(request.uri.queryParameters);
        auth.add(request.headers.value('Authorization'));
        request.response.headers.contentType = ContentType.json;
        request.response.write(jsonEncode({
          'server_instance_id': 'server-a',
          'platform': 'embedded',
          'active_profile': {},
          'location': {},
          'mode': {},
          'review': {},
          'transitions': [],
          'profiles': [],
          'scenes': [],
          'input_bindings': [],
          'capabilities': {
            'features': [if (!legacy) RhythmFeature.stateIncludesV1]
          },
          if (!legacy)
            'state_scope': {
              'schema_version': 1,
              'included': ['base', 'controls', 'configuration'],
              'nodes': 'controls'
            },
          'nodes': [
            {'id': 'room', 'kind': 'room'},
            if (legacy)
              {'id': 'bulb', 'kind': 'light_device', 'parent_id': 'room'}
          ],
        }));
        await request.response.close();
      });
      final connection = RhythmConnection();
      final helloFuture = connection.helloEvents.first;
      await connection.connect('127.0.0.1',
          port: server.port, authToken: 'test-token', authoritative: true);
      final hello = await helloFuture;
      expect(connection.connected, isTrue);
      expect(queries, [
        {'include': 'controls,configuration', 'authoritative': 'true'}
      ]);
      expect(auth, ['Bearer test-token']);
      expect(hello.nodes.length, legacy ? 2 : 1);
      expect(hello.stateScope == null, legacy);
      await connection.pollNow();
      expect(queries.last, legacy ? isEmpty : {'scope': 'controls'});
      connection.setDeviceDetailsNodes(['room', 'bulb']);
      await connection.pollNow();
      expect(queries.last, isEmpty,
          reason: 'an open detail surface keeps fallback polling current');
      connection.setDeviceDetailsNodes(null);
      await connection.pollNow();
      expect(queries.last, legacy ? isEmpty : {'scope': 'controls'});
      connection.dispose();
      await server.close(force: true);
    }
  });

  test('explicit base/detail reads distinguish empty, malformed and failed',
      () async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    var status = 200;
    var response = <String, dynamic>{
      'state_scope': {
        'schema_version': 1,
        'included': ['base'],
        'nodes': 'none'
      }
    };
    Map<String, String>? query;
    server.listen((request) async {
      query = request.uri.queryParameters;
      request.response.statusCode = status;
      request.response.headers.contentType = ContentType.json;
      request.response.write(jsonEncode(response));
      await request.response.close();
    });
    addTearDown(() => server.close(force: true));
    final api = RhythmRuntimeApi.fromBaseUrl(
        baseUrl: 'http://127.0.0.1:${server.port}/');
    expect((await api.getState()).stateScope!.nodes, 'none');
    expect(query, {'include': 'base'});
    response = {
      'state_scope': {
        'schema_version': 1,
        'included': ['base', 'nodes'],
        'nodes': 'all'
      },
      'nodes': []
    };
    expect(
        (await api.getState(
                include: {RhythmStateInclude.nodes}, authoritative: true))
            .nodes,
        isEmpty);
    expect(query, {'include': 'nodes', 'authoritative': 'true'});
    response.remove('nodes');
    await expectLater(api.getState(include: {RhythmStateInclude.nodes}),
        throwsFormatException);
    status = 403;
    await expectLater(api.getState(), throwsA(isA<DioException>()));
  });
}
