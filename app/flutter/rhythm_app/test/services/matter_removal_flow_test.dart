import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/matter_removal_flow.dart';

void main() {
  group('runMatterRemovalFlow', () {
    test('attempts a graceful unpair first and stops on success', () async {
      final forcedAttempts = <bool>[];

      final outcome = await runMatterRemovalFlow(
        unpair: ({required bool force}) async {
          forcedAttempts.add(force);
          return {'status': 'complete'};
        },
        confirmForceRemove: (_) async =>
            fail('graceful success must not prompt for force removal'),
      );

      expect(outcome, MatterRemovalOutcome.removed);
      expect(forcedAttempts, [false]);
    });

    test('never forces without user confirmation', () async {
      final forcedAttempts = <bool>[];
      final promptedErrors = <String>[];

      final outcome = await runMatterRemovalFlow(
        unpair: ({required bool force}) async {
          forcedAttempts.add(force);
          return {'status': 'failed', 'error': 'device unreachable'};
        },
        confirmForceRemove: (error) async {
          promptedErrors.add(error);
          return false;
        },
      );

      expect(outcome, MatterRemovalOutcome.cancelled);
      expect(forcedAttempts, [false]);
      expect(promptedErrors, ['device unreachable']);
    });

    test('escalates to force only after confirmation', () async {
      final forcedAttempts = <bool>[];

      final outcome = await runMatterRemovalFlow(
        unpair: ({required bool force}) async {
          forcedAttempts.add(force);
          return force
              ? {'status': 'complete'}
              : {'status': 'failed', 'error': 'device unreachable'};
        },
        confirmForceRemove: (_) async => true,
      );

      expect(outcome, MatterRemovalOutcome.removed);
      expect(forcedAttempts, [false, true]);
    });

    test('re-prompts when a confirmed force attempt also fails', () async {
      final forcedAttempts = <bool>[];
      var prompts = 0;

      final outcome = await runMatterRemovalFlow(
        unpair: ({required bool force}) async {
          forcedAttempts.add(force);
          return null;
        },
        confirmForceRemove: (_) async {
          prompts += 1;
          return prompts < 2;
        },
      );

      expect(outcome, MatterRemovalOutcome.cancelled);
      expect(forcedAttempts, [false, true]);
      expect(prompts, 2);
    });

    test('maps a missing result to the no-response error message', () async {
      final promptedErrors = <String>[];

      await runMatterRemovalFlow(
        unpair: ({required bool force}) async => null,
        confirmForceRemove: (error) async {
          promptedErrors.add(error);
          return false;
        },
      );

      expect(promptedErrors, [matterRemovalNoResponseError]);
    });
  });
}
