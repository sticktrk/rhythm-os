class MatterBulbTestStep {
  const MatterBulbTestStep({
    required this.id,
    required this.title,
    required this.prompt,
    required this.runLabel,
    this.preparation,
    this.requiresXy = false,
  });

  final String id;
  final String title;
  final String prompt;
  final String runLabel;
  final String? preparation;
  final bool requiresXy;
}

const defaultMatterBulbCommandSpacingMs = 250;

const matterBulbTestSteps = <MatterBulbTestStep>[
  MatterBulbTestStep(
    id: 'identify',
    title: 'Identify',
    prompt: 'Did this bulb blink?',
    runLabel: 'Blink bulb',
  ),
  MatterBulbTestStep(
    id: 'turn_off',
    title: 'Baseline Off',
    prompt: 'Did the bulb turn fully off?',
    runLabel: 'Turn off bulb',
  ),
  MatterBulbTestStep(
    id: 'brightness_without_on',
    title: 'Brightness Without On',
    prompt: 'Did only the brightness command turn the bulb on?',
    runLabel: 'Test brightness',
    preparation: 'The tester turns the bulb off first.',
  ),
  MatterBulbTestStep(
    id: 'brightness_with_on',
    title: 'Explicit On',
    prompt: 'Did explicit on followed by brightness work?',
    runLabel: 'Test explicit on',
    preparation: 'The tester turns the bulb off first.',
  ),
  MatterBulbTestStep(
    id: 'level_move_to_level',
    title: 'MoveToLevel',
    prompt: 'Did plain MoveToLevel turn the bulb on or visibly set brightness?',
    runLabel: 'Test MoveToLevel',
    preparation: 'The tester turns the bulb off first.',
  ),
  MatterBulbTestStep(
    id: 'level_move_to_level_with_onoff',
    title: 'MoveToLevelWithOnOff',
    prompt: 'Did MoveToLevelWithOnOff turn the bulb on and set brightness?',
    runLabel: 'Test MTL OnOff',
    preparation: 'The tester turns the bulb off first.',
  ),
  MatterBulbTestStep(
    id: 'level_step_with_onoff',
    title: 'StepWithOnOff',
    prompt: 'Did StepWithOnOff turn the bulb on or visibly step brightness?',
    runLabel: 'Test Step OnOff',
    preparation: 'The tester turns the bulb off first.',
  ),
  MatterBulbTestStep(
    id: 'level_step',
    title: 'Step',
    prompt: 'Did the plain Step command visibly lower brightness?',
    runLabel: 'Test Step',
    preparation: 'The tester turns the bulb on at neutral white first.',
  ),
  MatterBulbTestStep(
    id: 'dim_low',
    title: 'Low Dim',
    prompt: 'Did it stay on at a stable low dim level?',
    runLabel: 'Test low dim',
    preparation: 'The tester turns the bulb on at neutral white first.',
  ),
  MatterBulbTestStep(
    id: 'dim_ramp',
    title: 'Dimming Ramp',
    prompt: 'Did it fade smoothly instead of jumping or failing?',
    runLabel: 'Test fade',
    preparation: 'The tester turns the bulb on at neutral white first.',
  ),
  MatterBulbTestStep(
    id: 'brightness_steps',
    title: 'Brightness Range',
    prompt: 'Were the low, medium, and full brightness steps visible?',
    runLabel: 'Test range',
    preparation: 'The tester turns the bulb on at neutral white first.',
  ),
  MatterBulbTestStep(
    id: 'on_level_restore',
    title: 'On Level Restore',
    prompt:
        'After dimming, turning off, and turning on, did it return at the low level?',
    runLabel: 'Test restore',
    preparation: 'The tester starts at neutral white and 10% brightness.',
  ),
  MatterBulbTestStep(
    id: 'power_on_behavior',
    title: 'Power-On Restore',
    prompt:
        'After setup, physically power-cycle the bulb. Did it return warm at about 50%?',
    runLabel: 'Set power test',
    preparation: 'The tester sets warm white at 50% first.',
  ),
  MatterBulbTestStep(
    id: 'color_temperature_warm',
    title: 'Warm White',
    prompt: 'Did the bulb become warm white?',
    runLabel: 'Test warm white',
    preparation: 'The tester resets the bulb to neutral white first.',
  ),
  MatterBulbTestStep(
    id: 'color_temperature_cool',
    title: 'Cool White',
    prompt: 'Did the bulb become cool white?',
    runLabel: 'Test cool white',
    preparation: 'The tester resets the bulb to neutral white first.',
  ),
  MatterBulbTestStep(
    id: 'hue_sat_red',
    title: 'Hue/Sat Red',
    prompt:
        'Did Hue/Saturation turn the bulb red? If red works, the other colors are assumed to work.',
    runLabel: 'Test HS red',
    preparation: 'The tester resets the bulb to neutral white first.',
  ),
  MatterBulbTestStep(
    id: 'ct_to_hue_sat',
    title: 'CT to Hue/Sat',
    prompt: 'Did it move from warm white into Hue/Saturation color?',
    runLabel: 'Test CT to HS',
    preparation: 'The tester resets the bulb to neutral white first.',
  ),
  MatterBulbTestStep(
    id: 'hue_sat_to_ct',
    title: 'Hue/Sat to CT',
    prompt: 'Did it move from Hue/Saturation color back to warm white?',
    runLabel: 'Test HS to CT',
    preparation: 'The tester resets the bulb to neutral white first.',
  ),
  MatterBulbTestStep(
    id: 'xy_red',
    title: 'XY Red',
    prompt:
        'Did XY turn the bulb red? If it failed or turned off, choose No and no more XY commands will run.',
    runLabel: 'Test XY red',
    preparation: 'The tester resets the bulb to neutral white first.',
  ),
  MatterBulbTestStep(
    id: 'ct_to_xy',
    title: 'CT to XY',
    prompt: 'Did it move from warm white into XY red?',
    runLabel: 'Test CT to XY',
    preparation: 'The tester resets the bulb to neutral white first.',
    requiresXy: true,
  ),
  MatterBulbTestStep(
    id: 'xy_to_ct',
    title: 'XY to CT',
    prompt: 'Did it move from XY color back to warm white?',
    runLabel: 'Test XY to CT',
    preparation: 'The tester resets the bulb to neutral white first.',
    requiresXy: true,
  ),
];

List<MatterBulbTestStep> activeMatterBulbTestSteps({
  required bool xyFailed,
}) {
  if (!xyFailed) return matterBulbTestSteps;
  return matterBulbTestSteps
      .where((step) => !step.requiresXy)
      .toList(growable: false);
}

List<String> skippedMatterBulbTestIds({required bool xyFailed}) {
  if (!xyFailed) return const [];
  return matterBulbTestSteps
      .where((step) => step.requiresXy)
      .map((step) => step.id)
      .toList(growable: false);
}
