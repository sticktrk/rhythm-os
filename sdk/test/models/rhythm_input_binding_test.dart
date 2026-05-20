import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  group('RhythmInputBinding', () {
    test('parses day sleep preset mode cycle bindings', () {
      final binding = RhythmInputBinding.fromJson({
        'id': 'day_sleep_toggle:button-1:on_press',
        'preset': 'day_sleep_toggle',
        'source_node_id': 'button-1',
        'trigger': {
          'kind': 'button',
          'button_action': 'on_press',
        },
        'action': {
          'kind': 'mode_cycle',
          'modes': ['day', 'sleep'],
          'transition': {'kind': 'auto'},
        },
        'enabled': true,
      });

      expect(binding.id, 'day_sleep_toggle:button-1:on_press');
      expect(binding.preset, RhythmInputBindingPreset.daySleepToggle);
      expect(binding.sourceNodeId, 'button-1');
      expect(binding.trigger.isButton, isTrue);
      expect(binding.trigger.buttonAction, RhythmButtonAction.onPress);
      expect(binding.action, isA<RhythmModeCycleAction>());

      final action = binding.action as RhythmModeCycleAction;
      expect(action.modes, [RhythmMode.day, RhythmMode.sleep]);
      expect(action.transition, isA<RhythmModeTransitionAuto>());
      expect(binding.toJson(), {
        'id': 'day_sleep_toggle:button-1:on_press',
        'preset': 'day_sleep_toggle',
        'source_node_id': 'button-1',
        'trigger': {
          'kind': 'button',
          'button_action': 'on_press',
        },
        'action': {
          'kind': 'mode_cycle',
          'modes': ['day', 'sleep'],
          'transition': {'kind': 'auto'},
        },
        'enabled': true,
      });
    });

    test('normalizes button action aliases', () {
      expect(RhythmButtonAction.fromString('on'), RhythmButtonAction.onPress);
      expect(RhythmButtonAction.fromString('off'), RhythmButtonAction.offPress);
      expect(
        RhythmButtonAction.fromString('toggle'),
        RhythmButtonAction.toggle,
      );
    });

    test('keeps unknown preset and action values for forward compatibility',
        () {
      final binding = RhythmInputBinding.fromJson({
        'id': 'future-binding',
        'preset': 'future_preset',
        'source_node_id': 'button-2',
        'trigger': {
          'kind': 'button',
          'button_action': 'future_press',
        },
        'action': {
          'kind': 'future_action',
          'payload': {'x': 1},
        },
      });

      expect(binding.preset?.wireValue, 'future_preset');
      expect(binding.trigger.buttonAction?.wireValue, 'future_press');
      expect(binding.action, isA<RhythmRawAutomationAction>());
      expect(binding.action.toJson(), {
        'kind': 'future_action',
        'payload': {'x': 1},
      });
    });
  });

  group('RhythmAutomationAction', () {
    test('serializes mode set with exact transition', () {
      final action = RhythmModeSetAction(
        mode: RhythmMode.sleep,
        transition: const RhythmModeTransitionSelection.exact('day_to_sleep'),
      );

      expect(action.toJson(), {
        'kind': 'mode_set',
        'mode': 'sleep',
        'transition': {
          'kind': 'exact',
          'id': 'day_to_sleep',
        },
      });
    });

    test('parses backward-compatible mode toggle actions', () {
      final action = RhythmAutomationAction.fromJson({
        'kind': 'mode_toggle',
        'first_mode': 'day',
        'second_mode': 'sleep',
        'transition': {'kind': 'none'},
      });

      expect(action, isA<RhythmModeToggleAction>());
      final toggle = action as RhythmModeToggleAction;
      expect(toggle.firstMode, RhythmMode.day);
      expect(toggle.secondMode, RhythmMode.sleep);
      expect(toggle.transition, isA<RhythmModeTransitionNone>());
    });
  });
}
