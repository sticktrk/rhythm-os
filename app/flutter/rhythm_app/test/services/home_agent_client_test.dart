import 'dart:typed_data';

import 'package:dio/dio.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/home_agent_client.dart';

void main() {
  final requests = <RequestOptions>[];
  late HomeAgentClient client;
  var status = 200;
  late Map<String, dynamic> response;

  setUp(() {
    requests.clear();
    status = 200;
    response = {
      'session': 'fixture-session',
      'inference_provider': 'openai',
      'room_lookup': true,
      'scene_drafts': true,
    };
    final http = Dio();
    http.interceptors.add(InterceptorsWrapper(onRequest: (request, handler) {
      requests.add(request);
      final reply =
          Response(requestOptions: request, statusCode: status, data: response);
      if (status == 200) {
        handler.resolve(reply);
      } else {
        handler.reject(DioException(
            requestOptions: request,
            response: reply,
            type: DioExceptionType.badResponse));
      }
    }));
    client = HomeAgentClient(Uri.https('fixture.invalid'), 'home', 'hub',
        () => 'fixture-token', () => true,
        transport: http);
  });
  tearDown(() => client.close());

  test('text and photos use the OpenAI protocol; progress survives a rollback',
      () async {
    expect(await client.openSession(), isTrue);
    expect(client.supportsSceneDrafts && client.supportsRoomLookup, isTrue);
    await client.call('turn', {'messages': []});
    await client.call('photo', {}, Uint8List.fromList([1]));
    status = 404;
    await expectLater(client.call('photo', {}, Uint8List.fromList([2])),
        throwsA(isA<DioException>()));
    status = 200;
    await client.call('status');
    expect(requests.map((r) => r.uri.path),
        ['/v2/open', '/v2/turn', '/v2/photo', '/v2/photo', '/v1/status']);
    expect(requests[2].data, isA<FormData>());
    expect(requests.last.headers['x-rhythm-session'], 'fixture-session');
  });

  test('old or mismatched providers cannot receive resident input', () async {
    expect(await client.openSession(), isTrue);
    response.remove('inference_provider');
    expect(await client.openSession(), isFalse);
    expect(client.session, isNull);
    expect(client.supportsSceneDrafts || client.supportsRoomLookup, isFalse);
    await expectLater(client.call('turn', {'messages': []}), throwsStateError);
    status = 404;
    expect(await client.openSession(), isFalse);
    await expectLater(
        client.call('photo', {}, Uint8List.fromList([1])), throwsStateError);
    expect(requests.map((r) => r.uri.path), everyElement('/v2/open'));
    status = 403;
    response = {'error': 'membership_required'};
    await expectLater(client.openSession(), throwsA(isA<DioException>()));
  });
}
