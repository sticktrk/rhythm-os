import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:http/http.dart' as http;
import 'package:http/testing.dart';
import 'package:rhythm_app/services/apple_health_history_source.dart';
import 'package:rhythm_app/services/apple_health_sync_controller.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:supabase_flutter/supabase_flutter.dart';

const scope = AppleHealthScope(
    accountId: 'owner', homeId: 'home', serverId: 'box', nodeId: 'bedroom');
const bedtime = 1770073200000;
Map<String, dynamic> row(String id, String action, int at) => {
      'event_id': id,
      'node_id': 'bedroom',
      'action_id': action,
      'source_kind': 'switch',
      'epoch_ms': at,
    };

void main() {
  test('loads all retained pages and pairs across a page boundary', () async {
    final offsets = <int>[];
    final source =
        AppleHealthHistorySource(cloudPage: (_, offset, cutoff) async {
      offsets.add(offset);
      if (offset == 0) {
        return [
          for (var i = 0; i < 499; i++)
            row('wake-$i', 'sleep_off', bedtime - i - 1),
          row('sleep', 'sleep_on', bedtime),
        ];
      }
      return [row('wake', 'sleep_off', bedtime + 8 * 3600000)];
    });
    final snapshot = await source.load(scope, bedtime + 10 * 3600000);
    expect(offsets, [0, 500]);
    expect(snapshot.partial, isFalse);
    expect(
        RhythmSleepInterval.fromActivity(snapshot.events,
            nodeId: 'bedroom', nowEpochMs: bedtime + 10 * 3600000),
        hasLength(1));
  });

  test(
      'never projects truncated cloud pages; local fallback is visibly partial',
      () async {
    final source = AppleHealthHistorySource(
        cloudPage: (_, offset, __) async {
          if (offset != 0) throw StateError('offline');
          return [
            for (var i = 0; i < 500; i++) row('s$i', 'sleep_on', bedtime)
          ];
        },
        localHistory: (_) async => const RhythmHistory.empty());
    final snapshot = await source.load(scope, bedtime + 10 * 3600000);
    expect(snapshot.events, isEmpty);
    expect(snapshot.partial, isTrue);
    await expectLater(
        AppleHealthHistorySource(
                cloudPage: (_, __, ___) async => throw StateError('offline'))
            .load(scope, bedtime),
        throwsStateError);
  });

  test(
      'cloud query binds owner, home, durable server, physical input, target and cutoff',
      () async {
    final requests = <Uri>[];
    final client = SupabaseClient('https://example.invalid', 'synthetic-key',
        httpClient: MockClient((request) async {
      requests.add(request.url);
      return http.Response(jsonEncode([]), 200,
          request: request, headers: {'content-type': 'application/json'});
    }));
    addTearDown(client.dispose);
    await AppleHealthHistorySource.supabasePage(client)(scope, 0, bedtime);
    requests.clear();
    final source = AppleHealthHistorySource(
        cloudPage: AppleHealthHistorySource.supabasePage(client));
    final snapshot = await source.load(scope, bedtime);
    expect(snapshot.events, isEmpty);
    final query = requests.single.queryParameters;
    expect(query['user_id'], 'eq.owner');
    expect(query['home_id'], 'eq.home');
    expect(query['server_instance_id'], 'eq.box');
    expect(query['node_id'], 'eq.bedroom');
    expect(query['source_kind'], 'eq.switch');
    expect(query['epoch_ms'], 'lte.$bedtime');
    expect(query['order'], 'epoch_ms.asc.nullslast,event_id.asc.nullslast');
    expect(query['select'], isNot(contains('raw_activity')));
    expect(query['action_id'], contains('sleep_on'));
    await source.load(
        const AppleHealthScope(
            accountId: 'owner',
            homeId: 'home',
            serverId: 'box',
            nodeId: 'schedule:personal'),
        bedtime);
    expect(requests.last.queryParameters['node_id'], 'eq.global');
  });
}
