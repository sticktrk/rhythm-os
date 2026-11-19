import 'dart:convert';
import 'dart:io';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/screens/hubs/assistant_tools_screen.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

void main() {
  testWidgets(
      'the app exposes every read and temporary tool from the served hub contract',
      (tester) async {
    final raw = jsonDecode(
        File(Platform.environment['RHYTHM_ASSISTANT_CONTRACT']!)
            .readAsStringSync()) as Map<String, dynamic>;
    final contract = RhythmAssistantContract.fromJson(raw);
    await tester.pumpWidget(MaterialApp(
        home: AssistantToolsScreen(
      contract: contract,
      serverId: 'parity',
      targetChanges: ValueNotifier(true),
      isCurrentTarget: () => true,
      call: (_, __) =>
          throw StateError('Listing tools must not invoke hardware'),
    )));
    final select = tester.widget<DropdownButton<RhythmAssistantOperation>>(
        find.byType(DropdownButton<RhythmAssistantOperation>));
    final offered = select.items!.map((item) => item.value!.id);
    final ops = (raw['operations'] as List).cast<Map<String, dynamic>>();
    expect(
        offered,
        unorderedEquals(ops
            .where((row) =>
                row['tier'] == 'read' ||
                row['tier'] == 'temporary_light_output')
            .map((row) => row['id'])));
    for (final id in [
      RhythmAssistantOperationId.startMotionWalk,
      RhythmAssistantOperationId.inspectMotionWalk,
      RhythmAssistantOperationId.planMotionTarget,
      RhythmAssistantOperationId.applyMotionTarget
    ]) {
      expect(contract.supportsOperation(id), isTrue, reason: id);
    }
  });
}
