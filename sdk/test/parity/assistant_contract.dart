import 'dart:convert';
import 'dart:io';

import 'package:dio/dio.dart';
import 'package:mocktail/mocktail.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

import '../helpers/mock_dio.dart';

void main() {
  test('the SDK discovers and dispatches every operation the fresh hub served',
      () async {
    final raw = jsonDecode(
        File(Platform.environment['RHYTHM_ASSISTANT_CONTRACT']!)
            .readAsStringSync()) as Map<String, dynamic>;
    final dio = MockDio();
    final api = RhythmServerApi(dio);
    when(() => dio.get('api/assistant/contract')).thenAnswer(
        (_) async => Response(requestOptions: RequestOptions(), data: raw));
    when(() => dio.post(any(), data: any(named: 'data'))).thenAnswer(
        (call) async => Response(
            requestOptions: RequestOptions(),
            data: {'path': call.positionalArguments.single}));
    final contract = (await api.getAssistantContract())!;
    final ops = (raw['operations'] as List).cast<Map<String, dynamic>>();
    expect(contract.operations.length, ops.length);
    for (final row in ops) {
      final id = row['id'] as String;
      final op = contract.operation(id)!;
      expect(op.location, 'hub');
      expect(op.requiresOwnerApproval, row['requires_owner_approval']);
      expect(op.observesHousehold, row['observes_household']);
      expect(op.inputSchema, row['input_schema']);
      expect(op.resultSchema, row['result_schema']);
      expect(op.tier, isNot(RhythmAssistantTier.unknown), reason: id);
      if (row['tier'] == 'owner_only') {
        await expectLater(api.callAssistantOperation(contract, id, {}),
            throwsUnsupportedError);
        continue;
      }
      final args = <String, dynamic>{};
      if (row['tier'] == 'persistent_change') {
        await expectLater(api.callAssistantOperation(contract, id, args),
            throwsArgumentError);
        args['confirmed'] = true;
      }
      final result = await api.callAssistantOperation(contract, id, args);
      expect('/${result['path']}', row['invocation_path'], reason: id);
    }
  });
}
