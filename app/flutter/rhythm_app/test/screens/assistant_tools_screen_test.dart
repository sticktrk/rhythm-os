import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/screens/hubs/assistant_tools_screen.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

void main() {
  testWidgets(
      'a newly advertised plan must be reviewed on the same hub before apply',
      (tester) async {
    final contract = RhythmAssistantContract.fromJson({
      'operations': [
        {
          'id': 'preview_test',
          'tier': 'read',
          'input_schema': {
            'type': 'object',
            'properties': {
              'label': {'type': 'string'}
            },
            'required': ['label']
          }
        },
        {'id': 'apply_test', 'tier': 'persistent_change'},
      ]
    });
    final active = ValueNotifier(true);
    final calls = <(String, Map<String, dynamic>)>[];
    await tester.pumpWidget(MaterialApp(
        home: AssistantToolsScreen(
            contract: contract,
            serverId: 'selected',
            targetChanges: active,
            isCurrentTarget: () => active.value,
            call: (id, args) async {
              calls.add((id, args));
              return id == 'preview_test'
                  ? {
                      'server_instance_id': 'selected',
                      'operation': 'apply_test',
                      'scope': 'Selected room',
                      'before': 'before',
                      'after': args['label'],
                      'apply_arguments': {
                        'plan_id': 'opaque',
                        'resource_digest': 'reviewed-hash'
                      }
                    }
                  : {'status': 'applied'};
            })));
    await tester
        .tap(find.byType(DropdownButtonFormField<RhythmAssistantOperation>));
    await tester.pumpAndSettle();
    expect(find.text('apply test'), findsNothing);
    await tester.tap(find.text('preview test').last);
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextFormField), 'new');
    Future<void> tap(String label) async {
      await tester.tap(find.text(label));
      await tester.pumpAndSettle();
    }

    await tap('Inspect or preview');
    expect(find.textContaining('Selected room'), findsOneWidget);
    expect(calls, hasLength(1));
    await tap('Cancel');
    expect(calls, hasLength(1));
    await tap('Inspect or preview');
    active.value = false;
    await tester.pumpAndSettle();
    expect(find.text('Confirm reviewed change'), findsNothing);
    active.value = true;
    await tester.pumpAndSettle();
    await tap('Confirm reviewed change');
    expect(calls.last.$1, 'apply_test');
    expect(calls.last.$2, {
      'plan_id': 'opaque',
      'resource_digest': 'reviewed-hash',
      'confirmed': true
    });
  });
}
