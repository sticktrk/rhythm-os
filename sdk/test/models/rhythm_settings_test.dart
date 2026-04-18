import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:test/test.dart';

void main() {
  group('RhythmSettings', () {
    test('parses the split settings payload', () {
      final settings = RhythmSettings.fromJson({
        'power_save': true,
      });

      expect(settings.powerSave, isTrue);
    });

    test('uses safe defaults for missing fields', () {
      final settings = RhythmSettings.fromJson({});

      expect(settings.powerSave, isFalse);
    });
  });

  group('RhythmModeTransitionConfig', () {
    test('parses saved transitions with trigger objects', () {
      final transition = RhythmModeTransitionConfig.fromJson({
        'id': 'day_to_sleep_nautical_twilight',
        'label': 'Day to Sleep (Nautical Twilight)',
        'from_mode': 'day',
        'to_mode': 'sleep',
        'trigger': {
          'kind': 'solar',
          'event': 'nautical_twilight',
        },
        'duration_ms': 5000,
        'preserve_hard_off': true,
      });

      expect(transition.id, 'day_to_sleep_nautical_twilight');
      expect(transition.label, 'Day to Sleep (Nautical Twilight)');
      expect(transition.fromMode, RhythmMode.day);
      expect(transition.toMode, RhythmMode.sleep);
      expect(transition.trigger.kind, 'solar');
      expect(transition.trigger.event, 'nautical_twilight');
      expect(transition.duration, isA<TransitionDurationFixed>());
      expect(transition.durationMs, 5000);
      expect(transition.preserveHardOff, isTrue);
      expect(transition.toJson(), {
        'id': 'day_to_sleep_nautical_twilight',
        'label': 'Day to Sleep (Nautical Twilight)',
        'from_mode': 'day',
        'to_mode': 'sleep',
        'trigger': {
          'kind': 'solar',
          'event': 'nautical_twilight',
        },
        'duration_ms': 5000,
        'preserve_hard_off': true,
      });
    });

    test('accepts legacy string triggers as solar events', () {
      final transition = RhythmModeTransitionConfig.fromJson({
        'from_mode': 'sleep',
        'to_mode': 'day',
        'trigger': 'sunrise',
        'duration_ms': 3000,
        'preserve_hard_off': false,
      });

      expect(transition.trigger.kind, 'solar');
      expect(transition.trigger.event, 'sunrise');
    });

    test('parses scheduled triggers with local wall-clock time', () {
      final transition = RhythmModeTransitionConfig.fromJson({
        'from_mode': 'day',
        'to_mode': 'sleep',
        'trigger': {
          'kind': 'scheduled',
          'time': '22:00',
        },
        'duration_ms': 5000,
        'preserve_hard_off': true,
      });

      expect(transition.trigger.kind, 'scheduled');
      expect(transition.trigger.time, '22:00');
      expect(transition.toJson()['trigger'], {
        'kind': 'scheduled',
        'time': '22:00',
      });
    });

    test('parses auto duration object', () {
      final transition = RhythmModeTransitionConfig.fromJson({
        'from_mode': 'sleep',
        'to_mode': 'day',
        'trigger': {'kind': 'solar', 'event': 'sunrise'},
        'duration_ms': {'mode': 'auto'},
        'preserve_hard_off': true,
      });

      expect(transition.duration, isA<TransitionDurationAuto>());
      expect(transition.duration.isAuto, isTrue);
      expect(transition.durationMs, 0);
      expect(transition.toJson()['duration_ms'], {'mode': 'auto'});
    });

    test('fixed duration round-trips as bare number', () {
      final transition = RhythmModeTransitionConfig.fromJson({
        'from_mode': 'day',
        'to_mode': 'sleep',
        'duration_ms': 10000,
        'preserve_hard_off': false,
      });

      expect(transition.duration, isA<TransitionDurationFixed>());
      expect(transition.durationMs, 10000);
      expect(transition.toJson()['duration_ms'], 10000);
    });
  });

  group('RhythmModeResource', () {
    test('parses the split mode payload', () {
      final mode = RhythmModeResource.fromJson({
        'active': 'sleep',
        'last_change': {
          'cause': 'schedule',
          'transition_id': 'day_to_sleep_nautical_twilight',
          'epoch_ms': 1700000000000,
        },
        'configs': [
          {
            'mode': 'day',
            'active_profile_id': 'rhythm',
            'idle_profile_id': 'day_idle',
          },
          {
            'mode': 'sleep',
            'active_profile_id': 'sleep',
            'idle_profile_id': 'sleep_idle',
          },
        ],
      });

      expect(mode.active, RhythmMode.sleep);
      expect(mode.lastChange, isNotNull);
      expect(mode.lastChange!.cause, 'schedule');
      expect(mode.lastChange!.transitionId, 'day_to_sleep_nautical_twilight');
      expect(mode.lastChange!.epochMs, 1700000000000);
      expect(mode.configs, hasLength(2));
      expect(mode.activeConfig?.activeProfileId, 'sleep');
      expect(mode.activeConfig?.idleProfileId, 'sleep_idle');
    });

    test('preserves null idle_profile_id for synthesized fallback idle', () {
      final mode = RhythmModeResource.fromJson({
        'active': 'day',
        'configs': [
          {
            'mode': 'day',
            'active_profile_id': 'rhythm',
            'idle_profile_id': null,
          },
        ],
      });

      expect(mode.activeConfig?.idleProfileId, isNull);
      expect(mode.configs.single.toJson()['idle_profile_id'], isNull);
    });

    test('returns null for missing mode configs', () {
      final mode = RhythmModeResource.fromJson({
        'active': 'day',
        'configs': const [],
      });

      expect(mode.activeConfig, isNull);
      expect(mode.configFor(RhythmMode.sleep), isNull);
    });
  });

  group('RhythmProfiles', () {
    test('parses the profiles list payload', () {
      final profiles = RhythmProfiles.fromJson({
        'profiles': [
          {
            'id': 'day_idle',
            'name': 'Day Idle',
            'curve': {'type': 'inherit-active'},
            'min_brightness': 1,
            'max_brightness': 1,
            'min_color_temp': 0,
            'max_color_temp': 0,
            'max_dim_steps': 1,
          },
          {
            'id': 'rhythm',
            'name': 'Day',
            'curve': {'type': 'super-gaussian'},
            'min_brightness': 1,
            'max_brightness': 100,
            'min_color_temp': 2200,
            'max_color_temp': 6500,
            'max_dim_steps': 12,
            'rhythm_interval_secs': 60,
          },
          {
            'id': 'sleep',
            'name': 'Sleep',
            'curve': {
              'type': 'super-gaussian',
              'direct_color': {
                'rgb': {'r': 255, 'g': 147, 'b': 41},
                'xy': {'x': 0.675, 'y': 0.322},
              },
            },
            'min_brightness': 10,
            'max_brightness': 40,
            'min_color_temp': 500,
            'max_color_temp': 500,
            'max_dim_steps': 12,
            'fade_ms': 500,
            'motion_timeout_secs': 1200,
            'rhythm_interval_secs': 90,
          },
          {
            'id': 'sleep_idle',
            'name': 'Sleep Idle',
            'curve': {
              'type': 'constant',
              'brightness': 1,
              'color_temp': 0,
            },
            'min_brightness': 1,
            'max_brightness': 1,
            'min_color_temp': 0,
            'max_color_temp': 0,
            'max_dim_steps': 1,
          },
        ],
      });

      expect(profiles.profiles.map((profile) => profile.id), [
        'day_idle',
        'rhythm',
        'sleep',
        'sleep_idle',
      ]);
      expect(profiles.profiles.first.name, 'Day Idle');
      expect(profiles.profiles[2].fadeMs, 500);
      expect(profiles.profiles[2].motionTimeoutSecs, 1200);
    });

    test('treats null values as absent', () {
      final profiles = RhythmProfiles.fromJson({
        'profiles': null,
      });

      expect(profiles.profiles, isEmpty);
    });
  });
}
