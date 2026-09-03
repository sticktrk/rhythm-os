import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/bulb_audition_script.dart';

void main() {
  test('audition is scenario-based and covers runtime and subscriptions', () {
    final ids = bulbAuditionScenarios.map((scenario) => scenario.id).toList();

    expect(ids, hasLength(16));
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
      'command_spacing',
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
    expect(
      bulbAuditionScenarios
          .firstWhere(
              (scenario) => scenario.id == 'subscription_external_change')
          .preparation,
      startsWith('Start this check'),
    );
  });

  test('one-release source aliases preserve the canonical script', () {
    expect(matterBulbTestSteps, same(bulbAuditionScenarios));
    expect(activeMatterBulbTestSteps(xyFailed: true), bulbAuditionScenarios);
    expect(skippedMatterBulbTestIds(xyFailed: true), isEmpty);
  });

  test('operator answers become sourced typed profile fields', () {
    final profile = applyBulbAuditionAnswersToControlProfile(
      {
        'turn_on': 'stage_color_then_level_with_on_off',
        'power_on_behavior': 'unknown',
        'on_restores_previous': false,
        'supports_transition': true,
        'source': {
          'turn_on': 'safe_default',
          'power_on_behavior': 'safe_default',
          'on_restores_previous': 'safe_default',
          'min_brightness': 'safe_default',
          'supports_transition': 'safe_default',
        },
      },
      const {
        'turn_on_from_off': false,
        'dim_floor': false,
        'dim_ramp': false,
        'power_cycle_then_tick': true,
        'off_then_on_restore': false,
      },
    );

    expect(profile['turn_on'], 'explicit_on_first');
    expect(profile['min_brightness'], 10);
    expect(profile['supports_transition'], isFalse);
    expect(profile['power_on_behavior'], 'restore_previous');
    expect(profile['on_restores_previous'], isTrue);
    expect(profile['source'], containsPair('turn_on', 'audition'));
    expect(profile['source'], containsPair('min_brightness', 'audition'));
    expect(
      profile['source'],
      containsPair('supports_transition', 'audition'),
    );
    expect(
      profile['source'],
      containsPair('power_on_behavior', 'audition'),
    );
    expect(
      profile['source'],
      containsPair('on_restores_previous', 'audition'),
    );
    expect(controlProfileHasAuditionEvidence(profile), isTrue);
  });

  test('accepted turn-on override outranks a negative default-plan answer', () {
    final profile = applyBulbAuditionAnswersToControlProfile(
      {
        'turn_on': 'level_with_on_off_then_color',
        'source': {'turn_on': 'try_with'},
      },
      const {'turn_on_from_off': false},
      acceptedOverrideFields: const {'turn_on'},
    );

    expect(profile['turn_on'], 'level_with_on_off_then_color');
    expect(profile['source'], containsPair('turn_on', 'try_with'));
  });
}
