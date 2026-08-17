import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  test('move plan preserves a nullable unassigned source in apply JSON', () {
    final plan = RhythmAssistantMovePlan.fromJson({
      'schema_version': 1,
      'plan_id': 'move-unassigned',
      'correlation_id': 'setup-unassigned',
      'operation': RhythmAssistantOperationId.applyMoveDeviceRoomPlan,
      'contract_sha256': 'contract',
      'server_instance_id': 'srv-1',
      'topology_resource_sha256': 'topology',
      'device': {'id': 'light-1', 'name': 'Light'},
      'from_room': null,
      'to_room': {'id': 'foyer', 'name': 'Foyer'},
      'resulting_placement': 'user_override',
      'requires_confirmation': true,
    });

    expect(plan.fromRoom, isNull);
    expect(plan.toApplyJson()['from_room_id'], isNull);
    expect(
      plan.toApplyJson()['operation'],
      RhythmAssistantOperationId.applyMoveDeviceRoomPlan,
    );
  });

  test('structured stale error remains machine-readable', () {
    final error = RhythmAssistantError.fromJson({
      'error': {
        'code': 'topology_changed',
        'message': 'Refresh and create a new plan',
        'retryable': true,
        'mutation_may_have_applied': false,
      },
    });

    expect(error.code, 'topology_changed');
    expect(error.retryable, isTrue);
    expect(error.mutationMayHaveApplied, isFalse);
  });
}
