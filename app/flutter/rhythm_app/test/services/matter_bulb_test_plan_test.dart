import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/services/matter_bulb_test_plan.dart';

void main() {
  test('plan keeps CT coverage and removes redundant colors and rapid tests',
      () {
    final ids = matterBulbTestSteps.map((step) => step.id).toList();

    expect(ids, hasLength(21));
    expect(
      ids,
      containsAll([
        'color_temperature_warm',
        'color_temperature_cool',
        'ct_to_hue_sat',
        'hue_sat_to_ct',
        'ct_to_xy',
        'xy_to_ct',
      ]),
    );
    for (final removedId in [
      'xy_green',
      'xy_blue',
      'hue_sat_green',
      'hue_sat_blue',
      'rapid_commands',
    ]) {
      expect(ids, isNot(contains(removedId)));
    }
    expect(ids.where((id) => id.startsWith('rapid_')), isEmpty);
    expect(defaultMatterBulbCommandSpacingMs, 250);
    expect(ids.indexOf('hue_sat_red'), lessThan(ids.indexOf('xy_red')));
  });

  test('first XY failure ends the plan without another XY-dependent command',
      () {
    final active = activeMatterBulbTestSteps(xyFailed: true);
    final activeIds = active.map((step) => step.id).toList();

    expect(activeIds, hasLength(19));
    expect(activeIds.last, 'xy_red');
    expect(activeIds, isNot(contains('ct_to_xy')));
    expect(activeIds, isNot(contains('xy_to_ct')));
    expect(
      skippedMatterBulbTestIds(xyFailed: true),
      ['ct_to_xy', 'xy_to_ct'],
    );
  });

  test('off-state tests explicitly own their preparation', () {
    const offStateIds = {
      'brightness_without_on',
      'brightness_with_on',
      'level_move_to_level',
      'level_move_to_level_with_onoff',
      'level_step_with_onoff',
    };

    for (final step in matterBulbTestSteps) {
      if (offStateIds.contains(step.id)) {
        expect(step.preparation, 'The tester turns the bulb off first.');
      }
    }
  });
}
