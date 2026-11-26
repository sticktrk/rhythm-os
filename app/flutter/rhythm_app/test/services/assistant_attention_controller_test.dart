import 'dart:async';

import 'package:dio/dio.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/assistant_attention_controller.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

class _Api extends RhythmAuthApi {
  _Api() : super(baseUrl: 'https://hub.example');
  List<RhythmAssistantConnection> connections = [];
  List<RhythmAssistantApproval> plans = [];
  int? errorStatus;
  int reads = 0;
  Completer<List<RhythmAssistantConnection>>? delayed;

  @override
  Future<List<RhythmAssistantConnection>> listAssistantConnections() async {
    reads++;
    if (errorStatus != null) {
      throw DioException(
          requestOptions: RequestOptions(),
          response: Response(
              requestOptions: RequestOptions(), statusCode: errorStatus));
    }
    return delayed?.future ?? Future.value(connections);
  }

  @override
  Future<List<RhythmAssistantApproval>> listAssistantApprovals() async => plans;
}

RhythmAssistantConnection request(String id) => RhythmAssistantConnection(
    id: id,
    destination: 'Home Assistant',
    kind: 'home_assistant',
    remote: false,
    expiresAt: DateTime.now().add(const Duration(minutes: 5)));

void main() {
  late AssistantAttentionController controller;
  late StreamController<void> events;
  late _Api api;
  bool current = true;
  Future<void> settle() => Future<void>.delayed(Duration.zero);
  setUp(() {
    controller = AssistantAttentionController();
    api = _Api();
    current = true;
    events = StreamController<void>.broadcast();
    controller.select(
        hubId: 'hub',
        api: api,
        isCurrentTarget: () => current,
        attentionEvents: events.stream);
  });
  tearDown(() async {
    controller.dispose();
    await events.close();
  });

  test('event and matching push reload the queue, with one prompt per request',
      () async {
    await settle();
    expect(controller.next, isNull);
    api.connections = [request('a'), request('b')];
    events.add(null);
    await settle();
    expect(controller.next?.id, 'a');
    controller.dismiss('a');
    expect(controller.next?.id, 'b');
    controller.dismiss('b');
    final reads = api.reads;
    controller.push('different');
    await settle();
    expect(api.reads, reads);
    controller.push('hub');
    await settle();
    expect(api.reads, reads + 1);
    expect(controller.next, isNull);
    controller.reviewAgain('a');
    expect(controller.next?.id, 'a');
  });

  test('plan events read the hub review and remove it after resolution',
      () async {
    await settle();
    api.plans = [
      const RhythmAssistantApproval(
          planId: 'plan',
          operation: 'plan_room_tuning',
          credentialId: 'client',
          review: {'after': 'updated'})
    ];
    events.add(null);
    await settle();
    expect((controller.next as PlanAttention).plan.review['after'], 'updated');
    api.plans = [];
    events.add(null);
    await settle();
    expect(controller.contains('plan'), isFalse);
    expect(controller.next, isNull);
  });

  for (final status in [401, 403, 404]) {
    test('status $status clears requests', () async {
      await settle();
      api.connections = [request('a')];
      await controller.refresh();
      expect(controller.next, isA<ConnectionAttention>());
      api.errorStatus = status;
      await controller.refresh();
      expect(controller.next, isNull);
    });
  }

  test(
      'hub switching invalidates an outstanding queue read and decision target',
      () async {
    await settle();
    api.delayed = Completer<List<RhythmAssistantConnection>>();
    final old = controller.refresh();
    current = false;
    expect(controller.api, isNull);
    controller.select(
        hubId: 'other',
        api: _Api(),
        isCurrentTarget: () => true,
        attentionEvents: events.stream);
    api.delayed!.complete([request('old-request')]);
    await old;
    expect(controller.hubId, 'other');
    expect(controller.next, isNull);
    expect(controller.contains('old-request'), isFalse);
  });
}
