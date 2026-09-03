import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/bulb_audition_script.dart';

void main() {
  test('audition is scenario-based and covers runtime and subscriptions', () {
    final ids = bulbAuditionScenarios.map((scenario) => scenario.id).toList();

    expect(ids, hasLength(15));
    expect(ids, [
      'preflight',
      'identify',
      'turn_on_from_off',
      'tick_while_on',
      'adaptive_white_route',
      'tick_while_off',
      'color_to_white_and_back',
      'off_then_on_restore',
      'power_cycle_then_tick',
      'subscription_establish',
      'subscription_external_change',
      'subscription_liveness',
      'dim_floor',
      'dim_ramp',
      'brightness_range',
    ]);
    expect(ids.where((id) => id.contains('rapid')), isEmpty);
    expect(bulbAuditionScenarios.first.operatorAnswer, isFalse);
    expect(
      bulbAuditionScenarios
          .firstWhere((scenario) => scenario.id == 'turn_on_from_off')
          .runLabel,
      'Run real plan',
    );
  });

  test('one-release source aliases preserve the canonical script', () {
    expect(matterBulbTestSteps, same(bulbAuditionScenarios));
    expect(activeMatterBulbTestSteps(xyFailed: true), bulbAuditionScenarios);
    expect(skippedMatterBulbTestIds(xyFailed: true), isEmpty);
  });
}
