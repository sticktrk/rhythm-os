class BulbAuditionScenario {
  const BulbAuditionScenario({
    required this.id,
    required this.title,
    required this.prompt,
    required this.runLabel,
    this.preparation,
    this.legacyTest,
    this.operatorAnswer = true,
  });

  final String id;
  final String title;
  final String prompt;
  final String runLabel;
  final String? preparation;
  final String? legacyTest;
  final bool operatorAnswer;
}

const bulbAuditionScenarios = <BulbAuditionScenario>[
  BulbAuditionScenario(
    id: 'preflight',
    title: 'Preflight',
    prompt: 'Review the bulb’s reported capabilities and current state.',
    runLabel: 'Read attributes',
    legacyTest: 'read_on_off',
    operatorAnswer: false,
  ),
  BulbAuditionScenario(
    id: 'identify',
    title: 'Identify',
    prompt: 'Did this bulb blink?',
    runLabel: 'Blink bulb',
    legacyTest: 'identify',
  ),
  BulbAuditionScenario(
    id: 'turn_on_from_off',
    title: 'Off → Warm 30%',
    prompt: 'Did it come on at the requested warm colour and level?',
    runLabel: 'Run real plan',
    preparation: 'Audition turns the bulb off, then runs the runtime plan.',
    legacyTest: 'brightness_without_on',
  ),
  BulbAuditionScenario(
    id: 'tick_while_on',
    title: 'On → Cool 80%',
    prompt: 'Did it change smoothly to the new target?',
    runLabel: 'Run tick',
    legacyTest: 'color_temperature_cool',
  ),
  BulbAuditionScenario(
    id: 'adaptive_white_route',
    title: 'Adaptive White Route',
    prompt:
        'At 2200, 2700, 4000, and 6500 K, did this bulb match the Hue neighbour?',
    runLabel: 'Rehearse four whites',
    preparation:
        'Place it beside a Hue bulb set to the same four targets. The route and source are recorded.',
  ),
  BulbAuditionScenario(
    id: 'tick_while_off',
    title: 'Tick While Off',
    prompt: 'Did it end in the intended off state?',
    runLabel: 'Run off tick',
    legacyTest: 'turn_off',
  ),
  BulbAuditionScenario(
    id: 'color_to_white_and_back',
    title: 'Colour ↔ White',
    prompt: 'Were both directions correct?',
    runLabel: 'Run mode changes',
    legacyTest: 'ct_to_hue_sat',
  ),
  BulbAuditionScenario(
    id: 'off_then_on_restore',
    title: 'Off Then New Target',
    prompt: 'Did it use the new target instead of restoring the old one?',
    runLabel: 'Test restore',
    legacyTest: 'on_level_restore',
  ),
  BulbAuditionScenario(
    id: 'power_cycle_then_tick',
    title: 'Power Cycle → Tick',
    prompt: 'After physically power-cycling it, did it recover to the target?',
    runLabel: 'Run recovery tick',
    preparation: 'Physically power-cycle the bulb before running.',
    legacyTest: 'power_on_behavior',
  ),
  BulbAuditionScenario(
    id: 'subscription_establish',
    title: 'Subscription Truth',
    prompt: 'Did the reported state agree with the bulb?',
    runLabel: 'Check subscription',
  ),
  BulbAuditionScenario(
    id: 'subscription_external_change',
    title: 'External Change',
    prompt: 'After changing it from another controller, was a report received?',
    runLabel: 'Observe change',
    preparation: 'For a multi-admin bulb, change it from its other app.',
  ),
  BulbAuditionScenario(
    id: 'subscription_liveness',
    title: 'Subscription Liveness',
    prompt: 'Did subscription reporting remain live?',
    runLabel: 'Check liveness',
  ),
  BulbAuditionScenario(
    id: 'dim_floor',
    title: 'Dim Floor',
    prompt: 'Did it stay on at a stable low level?',
    runLabel: 'Test low dim',
    legacyTest: 'dim_low',
  ),
  BulbAuditionScenario(
    id: 'dim_ramp',
    title: 'Dimming Ramp',
    prompt: 'Did it fade smoothly instead of jumping?',
    runLabel: 'Test ramp',
    legacyTest: 'dim_ramp',
  ),
  BulbAuditionScenario(
    id: 'brightness_range',
    title: 'Brightness Range',
    prompt: 'Were low, medium, and full brightness distinct?',
    runLabel: 'Test range',
    legacyTest: 'brightness_steps',
  ),
];

// One-release source aliases keep existing focused tests and downstream
// imports compiling while the canonical owner is Bulb Audition.
typedef MatterBulbTestStep = BulbAuditionScenario;
const matterBulbTestSteps = bulbAuditionScenarios;

List<MatterBulbTestStep> activeMatterBulbTestSteps({
  required bool xyFailed,
}) =>
    bulbAuditionScenarios;

List<String> skippedMatterBulbTestIds({required bool xyFailed}) => const [];
