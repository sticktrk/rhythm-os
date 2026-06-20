import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/features/circadian_expert/circadian_expert_screen.dart';

const _testFilterPresets = <String, Object?>{
  'Standard': <String, Object?>{
    'at_dim': 100,
    'at_bright': 100,
    'off_threshold': 0,
  },
  'Overhead': <String, Object?>{
    'at_dim': 0,
    'at_bright': 100,
    'off_threshold': 3,
  },
  'Task': <String, Object?>{
    'at_dim': 80,
    'at_bright': 120,
    'off_threshold': 4,
  },
};

Map<String, Object?> _runtimeSettings(Map<String, Object?> values) => {
      'off_threshold': 0,
      'filter_presets': _testFilterPresets,
      'card_freshness_minutes': 15,
      'boost_default': 30,
      'tick_repeat_after_user_action': 10,
      'tick_repeat_after_autonomous_change': 3,
      'periodic_refresh_interval_minutes': 15,
      'duration_picker_presets': '5,60,240,1440,10080,forever',
      'default_pause_duration_minutes': 240,
      'default_freeze_duration_minutes': 60,
      'default_boost_duration_minutes': 5,
      'default_power_off_duration_minutes': 0,
      'confirm_zone_pushes': true,
      'rhythm_cursor_step_min': 5,
      'controls_pulse_window_hours': 6.0,
      'controls_recent_window_minutes': 5,
      'ct_compensation_enabled': false,
      'ct_compensation_begin_kelvin': 1650,
      'ct_compensation_end_kelvin': 2250,
      'ct_compensation_factor': 1.7,
      ...values,
    };

void main() {
  group('expert runtime snapshot mapping', () {
    test('loads top status from removed-project runtime now endpoint', () {
      final summary = expertServerSnapshotSummaryForTest({
        'current_hour': 9.5,
        'state': {
          'platform': 'removed-circadian-runtime',
        },
        'lighting': {
          'brightness': 64,
          'kelvin': 4120,
        },
      });

      expect(summary['path'], 'api/light-runtimes/removed-circadian/now');
      expect(summary['channel'], 'removed-circadian-runtime');
      expect(summary['server_hour'], 9.5);
      expect(summary['brightness'], 64);
      expect(summary['kelvin'], 4120);
    });
  });

  group('expert outdoor runtime mapping', () {
    test('loads outdoor status from removed-project runtime endpoints', () {
      final summary = expertOutdoorStatusSummaryForTest({
        'outdoor_normalized': 0.1,
        'source': 'override',
        'preferred_source': 'weather',
        'override': {
          'condition': 'rainy',
          'expires_in_minutes': 15.0,
        },
        'weather_condition': 'rainy',
        'lux_smoothed': null,
        'lux_learned_ceiling': null,
        'lux_learned_floor': null,
        'sun_elevation': null,
        'sensor_entity': null,
        'weather_groups': [
          {'key': 'sunny', 'label': 'Sunny', 'multiplier': 1.0},
          {'key': 'rainy', 'label': 'Rainy', 'multiplier': 0.2},
        ],
        'condition_multiplier': 0.2,
        'angle_factor': 0.5,
      });

      expect(summary['path'],
          'api/light-runtimes/removed-circadian/outdoor-status');
      expect(summary['refresh_path'],
          'api/light-runtimes/removed-circadian/refresh-outdoor');
      expect(summary['learn_path'],
          'api/light-runtimes/removed-circadian/learn-baselines');
      expect(summary['override_path'],
          'api/light-runtimes/removed-circadian/outdoor-override');
      expect(summary['outdoor_normalized'], 0.1);
      expect(summary['source'], 'override');
      expect(summary['preferred_source'], 'weather');
      expect(summary['override_condition'], 'rainy');
      expect(summary['override_expires_in_minutes'], 15.0);
      expect(summary['weather_condition'], 'rainy');
      expect(summary['weather_group_count'], 2);
      expect(summary['first_weather_group'], 'sunny');
      expect(summary['condition_multiplier'], 0.2);
      expect(summary['angle_factor'], 0.5);
      expect(summary['sun_elevation'], isNull);
    });

    test('throws on malformed runtime outdoor status payloads', () {
      expect(
        () => expertOutdoorStatusSummaryForTest({
          'source': 'weather',
          'preferred_source': 'weather',
          'weather_groups': [
            {'key': 'clear', 'label': 'Clear', 'multiplier': 1.0},
          ],
        }),
        throwsA(isA<FormatException>()),
      );
      expect(
        () => expertOutdoorStatusSummaryForTest({
          'outdoor_normalized': 0.5,
          'source': 'weather',
          'preferred_source': 'weather',
          'weather_groups': [
            {'key': 'clear', 'label': 'Clear'},
          ],
        }),
        throwsA(isA<FormatException>()),
      );
    });
  });

  group('expert settings server mapping', () {
    test('maps modern solar settings into removed-project expert UI keys', () {
      final values = expertSettingsValuesFromServerForTest(_runtimeSettings({
        'auto_update': false,
        'daily_sync_hour': 5,
        'daily_sync_minute': 30,
        'feedback_restrict_to_primary': true,
        'reach_feedback_enabled': false,
        'reach_daytime_threshold': 42,
        'freeze_feedback_enabled': false,
        'freeze_hold_at_dim': 200,
        'freeze_off_rise': 1100,
        'limit_bounce_enabled': false,
        'limit_bounce_max_percent': 22,
        'limit_bounce_min_percent': 11,
        'limit_warning_speed': 350,
        'alert_bounce_speed': 900,
        'boost_return_transition': 6500,
        'sun_saturation': 55,
        'outdoor_brightness_source': 'lux',
        'outdoor_lux_sensor': 'sensor.outdoor_illuminance',
        'weather_condition_map': {
          'sunny': 0.9,
          'cloudy': 0.25,
        },
        'lux_smoothing_interval': 600,
        'lux_learned_ceiling': 65000,
        'lux_learned_floor': 1200,
        'outdoor_refresh_interval': 45,
        'boost_default': 45,
        'confirm_zone_pushes': false,
        'duration_picker_presets': {
          'values': [10, 30, 0, 0, 0, 0, 0, 0],
          'len': 3,
        },
        'default_pause_duration_minutes': 60,
        'default_freeze_duration_minutes': 240,
        'default_boost_duration_minutes': 15,
        'default_power_off_duration_minutes': 0,
        'rhythm_cursor_step_min': 12,
        'card_freshness_minutes': 20,
        'controls_pulse_window_hours': 9,
        'controls_recent_window_minutes': 7,
        'activity_log_min_entries': 150,
        'activity_log_min_days': 14,
        'activity_log_flush_interval_minutes': 30,
        'tick_repeat_after_user_action': 12,
        'tick_repeat_after_autonomous_change': 4,
        'periodic_refresh_interval_minutes': 45,
        'experimental_tick_mode': 'skip',
        'multi_area_dispatch_stagger_ms': 0,
        'ct_compensation_enabled': true,
        'ct_compensation_begin_kelvin': 1600,
        'ct_compensation_end_kelvin': 2300,
        'ct_compensation_factor': 1.6,
        'two_step_turn_on': {
          'enabled': true,
          'kelvin_threshold': 220,
          'brightness_threshold_percent': 16,
          'delay_ms': 600,
        },
        'solar_color_rules': {
          'warm_night_enabled': true,
          'warm_night_mode': 'sunrise',
          'warm_night_target': 2500.0,
          'warm_night_start_minutes': -45,
          'warm_night_end_minutes': 75,
          'warm_night_fade_minutes': 30,
          'daylight_enabled': true,
          'daylight_cct': 5750.0,
          'daylight_start_minutes': 15,
          'daylight_end_minutes': -30,
          'daylight_fade_minutes': 90,
          'color_sensitivity': 1.25,
        },
      }));

      expect(values['auto_update'], false);
      expect(values['daily_sync_hour'], 5);
      expect(values['daily_sync_minute'], 30);
      expect(values['feedback_restrict_to_primary'], true);
      expect(values['reach_feedback_enabled'], false);
      expect(values['reach_daytime_threshold'], 42);
      expect(values['freeze_feedback_enabled'], false);
      expect(values['freeze_hold_at_dim'], 2.0);
      expect(values['freeze_off_rise'], 11.0);
      expect(values['limit_bounce_enabled'], false);
      expect(values['limit_bounce_max_percent'], 22);
      expect(values['limit_bounce_min_percent'], 11);
      expect(values['limit_warning_speed'], 3.5);
      expect(values['alert_bounce_speed'], 9.0);
      expect(values['boost_return_transition'], 65.0);
      expect(values['sun_saturation'], 55);
      expect(values['outdoor_brightness_source'], 'lux');
      expect(values['outdoor_lux_sensor'], 'sensor.outdoor_illuminance');
      expect(values['weather_condition_map'], {
        'sunny': 0.9,
        'cloudy': 0.25,
      });
      expect(values['lux_smoothing_interval'], 600);
      expect(values['lux_learned_ceiling'], 65000);
      expect(values['lux_learned_floor'], 1200);
      expect(values['outdoor_refresh_interval'], 45);
      expect(values['boost_default'], 45);
      expect(values['confirm_zone_pushes'], false);
      expect(values['duration_picker_presets'], {
        'values': [10, 30, 0, 0, 0, 0, 0, 0],
        'len': 3,
      });
      expect(values['default_pause_duration_minutes'], 60);
      expect(values['default_freeze_duration_minutes'], 240);
      expect(values['default_boost_duration_minutes'], 15);
      expect(values['default_power_off_duration_minutes'], 0);
      expect(values['rhythm_cursor_step_min'], 12);
      expect(values['card_freshness_minutes'], 20);
      expect(values['controls_pulse_window_hours'], 9);
      expect(values['controls_recent_window_minutes'], 7);
      expect(values['activity_log_min_entries'], 150);
      expect(values['activity_log_min_days'], 14);
      expect(values['activity_log_flush_interval_minutes'], 30);
      expect(values['tick_repeat_after_user_action'], 12);
      expect(values['tick_repeat_after_autonomous_change'], 4);
      expect(values['periodic_refresh_interval_minutes'], 45);
      expect(values['experimental_tick_mode'], 'skip');
      expect(values['multi_area_dispatch_stagger_ms'], 0);
      expect(values['ct_comp_enabled'], true);
      expect(values['ct_comp_begin'], 1600);
      expect(values['ct_comp_end'], 2300);
      expect(values['ct_comp_factor'], 1.6);
      expect(values['two_step_enabled'], true);
      expect(values['two_step_ct_threshold'], 220);
      expect(values['two_step_bri_threshold'], 16);
      expect(values['two_step_delay'], 6.0);
      expect(values['warm_night_enabled'], true);
      expect(values['warm_night_mode'], 'sunrise');
      expect(values['warm_night_target'], 2500.0);
      expect(values['warm_night_start'], -45);
      expect(values['warm_night_end'], 75);
      expect(values['warm_night_fade'], 30);
      expect(values['daylight_enabled'], true);
      expect(values['daylight_cct'], 5750.0);
      expect(values['daylight_start'], 15);
      expect(values['daylight_end'], -30);
      expect(values['daylight_fade'], 90);
      expect(values['color_sensitivity'], 1.25);
    });

    test('maps removed-project expert UI keys to modern server keys', () {
      void expectEntry(String key, Object? value, String expectedKey,
          Object? expectedValue) {
        final entry = modernExpertSettingsEntryForTest(key, value);
        expect(entry.key, expectedKey);
        expect(entry.value, expectedValue);
      }

      expectEntry('ct_comp_enabled', true, 'ct_compensation_enabled', true);
      expectEntry('ct_comp_begin', 1500, 'ct_compensation_begin_kelvin', 1500);
      expectEntry(
        'two_step_ct_threshold',
        240,
        'two_step_kelvin_threshold',
        240,
      );
      expectEntry(
        'two_step_bri_threshold',
        20,
        'two_step_brightness_threshold',
        20,
      );
      expectEntry('two_step_delay', 7, 'two_step_delay_ms', 700);
      expectEntry('freeze_hold_at_dim', 2, 'freeze_hold_at_dim', 200);
      expectEntry('freeze_off_rise', 11, 'freeze_off_rise', 1100);
      expectEntry('limit_warning_speed', 3.5, 'limit_warning_speed', 350);
      expectEntry('alert_bounce_speed', 9, 'alert_bounce_speed', 900);
      expectEntry(
        'boost_return_transition',
        65,
        'boost_return_transition',
        6500,
      );
      expectEntry('rhythm_cursor_step_min', 12, 'rhythm_cursor_step_min', 12);
      expectEntry(
        'card_freshness_minutes',
        20,
        'card_freshness_minutes',
        20,
      );
      expectEntry(
        'controls_pulse_window_hours',
        9,
        'controls_pulse_window_hours',
        9,
      );
      expectEntry(
        'controls_recent_window_minutes',
        7,
        'controls_recent_window_minutes',
        7,
      );
      expectEntry(
        'activity_log_min_entries',
        150,
        'activity_log_min_entries',
        150,
      );
      expectEntry('activity_log_min_days', 14, 'activity_log_min_days', 14);
      expectEntry(
        'activity_log_flush_interval_minutes',
        30,
        'activity_log_flush_interval_minutes',
        30,
      );
      expectEntry(
        'tick_repeat_after_user_action',
        12,
        'tick_repeat_after_user_action',
        12,
      );
      expectEntry(
        'tick_repeat_after_autonomous_change',
        4,
        'tick_repeat_after_autonomous_change',
        4,
      );
      expectEntry(
        'periodic_refresh_interval_minutes',
        45,
        'periodic_refresh_interval_minutes',
        45,
      );
      expectEntry(
        'experimental_tick_mode',
        'skip',
        'experimental_tick_mode',
        'skip',
      );
      expectEntry(
        'multi_area_dispatch_stagger_ms',
        0,
        'multi_area_dispatch_stagger_ms',
        0,
      );
      expectEntry(
        'outdoor_brightness_source',
        'weather',
        'outdoor_brightness_source',
        'weather',
      );
      expectEntry(
        'outdoor_lux_sensor',
        null,
        'outdoor_lux_sensor',
        null,
      );
      expectEntry(
        'weather_condition_map',
        {'rainy': 0.4},
        'weather_condition_map',
        {'rainy': 0.4},
      );
      expectEntry('daylight_start', 15, 'daylight_start', 15);
    });

    test('rejects malformed runtime weather condition maps', () {
      expect(
        () => expertSettingsValuesFromServerForTest(_runtimeSettings({
          'weather_condition_map': {'rainy': 'broken'},
        })),
        throwsA(isA<FormatException>()),
      );
      expect(
        () => expertSettingsValuesFromServerForTest(_runtimeSettings({
          'weather_condition_map': ['rainy', 0.2],
        })),
        throwsA(isA<FormatException>()),
      );
    });

    test('maps runtime light purpose presets for settings editing', () {
      final values = expertSettingsValuesFromServerForTest(_runtimeSettings({
        'filter_presets': {
          'Task': {'at_dim': 80, 'at_bright': 120, 'off_threshold': 4},
          'Standard': {'at_dim': 100, 'at_bright': 100},
          'Nightlight': {'at_dim': 40, 'at_bright': 0, 'off_threshold': 2},
        },
      }));

      expect(values['filter_presets'], {
        'Task': {'at_dim': 80, 'at_bright': 120, 'off_threshold': 4},
        'Standard': {'at_dim': 100, 'at_bright': 100},
        'Nightlight': {'at_dim': 40, 'at_bright': 0, 'off_threshold': 2},
      });
      expect(lightPurposePresetsFromConfigForTest(values), [
        {
          'name': 'Standard',
          'at_bright': 100,
          'at_dim': 100,
          'off_threshold': null,
        },
        {
          'name': 'Nightlight',
          'at_bright': 0,
          'at_dim': 40,
          'off_threshold': 2,
        },
        {
          'name': 'Task',
          'at_bright': 120,
          'at_dim': 80,
          'off_threshold': 4,
        },
      ]);

      expect(
        lightPurposePresetMapWithValueForTest(
          values,
          'Task',
          'at_bright',
          150,
        )['Task'],
        {'at_dim': 80, 'at_bright': 150, 'off_threshold': 4},
      );
    });

    test('rejects malformed runtime lab settings', () {
      for (final raw in [
        {
          'filter_presets': ['Standard']
        },
        {
          'filter_presets': {
            'Task': {'at_dim': 80, 'at_bright': 201},
          },
        },
        {
          'filter_presets': {
            'Task': {'at_dim': 80, 'at_bright': 120, 'off_threshold': 21},
          },
        },
        {
          'filter_presets': {
            '': {'at_dim': 80, 'at_bright': 120},
          },
        },
        {'off_threshold': '0'},
        {'experimental_tick_mode': 'unknown'},
        {'multi_area_dispatch_stagger_ms': '50'},
        {'card_freshness_minutes': 0},
        {'card_freshness_minutes': '15'},
        {'boost_default': 9},
        {'boost_default': '30'},
        {'tick_repeat_after_user_action': 61},
        {'tick_repeat_after_user_action': '10'},
        {'tick_repeat_after_autonomous_change': 61},
        {'tick_repeat_after_autonomous_change': '3'},
        {'periodic_refresh_interval_minutes': 1441},
        {'periodic_refresh_interval_minutes': '15'},
        {'duration_picker_presets': '10,broken,forever'},
        {
          'duration_picker_presets': {'values': '10,30'},
        },
        {'default_pause_duration_minutes': 10081},
        {'default_pause_duration_minutes': '240'},
        {'default_freeze_duration_minutes': 10081},
        {'default_freeze_duration_minutes': '60'},
        {'default_boost_duration_minutes': 10081},
        {'default_boost_duration_minutes': '5'},
        {'default_power_off_duration_minutes': 10081},
        {'default_power_off_duration_minutes': '0'},
        {'confirm_zone_pushes': 'true'},
        {'rhythm_cursor_step_min': 0},
        {'rhythm_cursor_step_min': '5'},
        {'controls_pulse_window_hours': 0.2},
        {'controls_pulse_window_hours': 49},
        {'controls_pulse_window_hours': '6'},
        {'controls_recent_window_minutes': 0},
        {'controls_recent_window_minutes': 121},
        {'controls_recent_window_minutes': '5'},
      ]) {
        expect(
          () => expertSettingsValuesFromServerForTest(_runtimeSettings(raw)),
          throwsA(isA<FormatException>()),
        );
      }
    });

    test('rejects runtime settings without card freshness window', () {
      expect(
        () => expertSettingsValuesFromServerForTest({
          'off_threshold': 0,
          'filter_presets': _testFilterPresets,
          'boost_default': 30,
        }),
        throwsA(isA<FormatException>()),
      );
    });

    test('rejects runtime settings without boost default', () {
      expect(
        () => expertSettingsValuesFromServerForTest({
          'off_threshold': 0,
          'filter_presets': _testFilterPresets,
          'card_freshness_minutes': 15,
          'tick_repeat_after_user_action': 10,
          'tick_repeat_after_autonomous_change': 3,
          'periodic_refresh_interval_minutes': 15,
          'duration_picker_presets': '5,60,240,1440,10080,forever',
          'default_pause_duration_minutes': 240,
          'default_freeze_duration_minutes': 60,
          'default_boost_duration_minutes': 5,
          'default_power_off_duration_minutes': 0,
          'confirm_zone_pushes': true,
          'rhythm_cursor_step_min': 5,
          'controls_pulse_window_hours': 6.0,
          'controls_recent_window_minutes': 5,
        }),
        throwsA(isA<FormatException>()),
      );
    });

    test('rejects runtime settings without G11 tick controls', () {
      final base = <String, Object?>{
        'off_threshold': 0,
        'filter_presets': _testFilterPresets,
        'card_freshness_minutes': 15,
        'boost_default': 30,
        'tick_repeat_after_user_action': 10,
        'tick_repeat_after_autonomous_change': 3,
        'periodic_refresh_interval_minutes': 15,
        'duration_picker_presets': '5,60,240,1440,10080,forever',
        'default_pause_duration_minutes': 240,
        'default_freeze_duration_minutes': 60,
        'default_boost_duration_minutes': 5,
        'default_power_off_duration_minutes': 0,
        'confirm_zone_pushes': true,
        'rhythm_cursor_step_min': 5,
        'controls_pulse_window_hours': 6.0,
        'controls_recent_window_minutes': 5,
      };
      for (final key in [
        'tick_repeat_after_user_action',
        'tick_repeat_after_autonomous_change',
        'periodic_refresh_interval_minutes',
      ]) {
        expect(
          () => expertSettingsValuesFromServerForTest(
            Map<String, Object?>.of(base)..remove(key),
          ),
          throwsA(isA<FormatException>()),
        );
      }
    });

    test('rejects runtime settings without duration defaults', () {
      final base = <String, Object?>{
        'off_threshold': 0,
        'filter_presets': _testFilterPresets,
        'card_freshness_minutes': 15,
        'boost_default': 30,
        'tick_repeat_after_user_action': 10,
        'tick_repeat_after_autonomous_change': 3,
        'periodic_refresh_interval_minutes': 15,
        'duration_picker_presets': '5,60,240,1440,10080,forever',
        'default_pause_duration_minutes': 240,
        'default_freeze_duration_minutes': 60,
        'default_boost_duration_minutes': 5,
        'default_power_off_duration_minutes': 0,
        'confirm_zone_pushes': true,
        'rhythm_cursor_step_min': 5,
        'controls_pulse_window_hours': 6.0,
        'controls_recent_window_minutes': 5,
      };
      for (final key in [
        'duration_picker_presets',
        'default_pause_duration_minutes',
        'default_freeze_duration_minutes',
        'default_boost_duration_minutes',
        'default_power_off_duration_minutes',
      ]) {
        expect(
          () => expertSettingsValuesFromServerForTest(
            Map<String, Object?>.of(base)..remove(key),
          ),
          throwsA(isA<FormatException>()),
        );
      }
    });

    test('rejects runtime settings without UI contract controls', () {
      final base = <String, Object?>{
        'off_threshold': 0,
        'filter_presets': _testFilterPresets,
        'card_freshness_minutes': 15,
        'boost_default': 30,
        'tick_repeat_after_user_action': 10,
        'tick_repeat_after_autonomous_change': 3,
        'periodic_refresh_interval_minutes': 15,
        'duration_picker_presets': '5,60,240,1440,10080,forever',
        'default_pause_duration_minutes': 240,
        'default_freeze_duration_minutes': 60,
        'default_boost_duration_minutes': 5,
        'default_power_off_duration_minutes': 0,
        'confirm_zone_pushes': true,
        'rhythm_cursor_step_min': 5,
        'controls_pulse_window_hours': 6.0,
        'controls_recent_window_minutes': 5,
      };
      for (final key in [
        'confirm_zone_pushes',
        'rhythm_cursor_step_min',
        'controls_pulse_window_hours',
        'controls_recent_window_minutes',
      ]) {
        expect(
          () => expertSettingsValuesFromServerForTest(
            Map<String, Object?>.of(base)..remove(key),
          ),
          throwsA(isA<FormatException>()),
        );
      }
    });

    test('loads light purpose names from runtime scope', () {
      expect(
        lightPurposePresetNamesFromScopeForTest({
          'light_filters': {
            'presets': {
              'Task': {'at_dim': 80, 'at_bright': 120, 'off_threshold': 4},
              'Standard': {'at_dim': 100, 'at_bright': 100},
              'Accent': {'at_dim': 50, 'at_bright': 50, 'off_threshold': 0},
            },
          },
        }),
        ['Standard', 'Accent', 'Task'],
      );
      expect(
        () => lightPurposePresetNamesFromScopeForTest({}),
        throwsA(isA<FormatException>()),
      );
    });

    test('updates every alias for a removed-project weather group', () {
      final updated = expertWeatherConditionMapWithGroupValueForTest(
        {'sunny': 1.0, 'cloudy': 0.3},
        'rainy',
        35,
      );

      expect(updated['sunny'], 1.0);
      expect(updated['cloudy'], 0.3);
      expect(updated['rainy'], 0.35);
      expect(updated['exceptional'], 0.35);
    });

    test('maps flat runtime-owned settings back into removed-project expert UI keys',
        () {
      final values = expertSettingsValuesFromServerForTest(_runtimeSettings({
        'ct_compensation_enabled': true,
        'ct_compensation_begin_kelvin': 1650,
        'ct_compensation_end_kelvin': 2250,
        'ct_compensation_factor': 1.7,
        'two_step_enabled': true,
        'two_step_kelvin_threshold': 250,
        'two_step_brightness_threshold': 18,
        'two_step_delay_ms': 850,
        'warm_night_enabled': true,
        'warm_night_mode': 'clock',
        'warm_night_start': -30,
        'warm_night_end': 90,
        'warm_night_fade': 45,
        'daylight_enabled': true,
        'daylight_start': 20,
        'daylight_end': -40,
        'daylight_fade': 100,
        'color_sensitivity': 1.4,
      }));

      expect(values['ct_comp_enabled'], true);
      expect(values['ct_comp_begin'], 1650);
      expect(values['ct_comp_end'], 2250);
      expect(values['ct_comp_factor'], 1.7);
      expect(values['two_step_enabled'], true);
      expect(values['two_step_ct_threshold'], 250);
      expect(values['two_step_bri_threshold'], 18);
      expect(values['two_step_delay'], 8.5);
      expect(values['warm_night_enabled'], true);
      expect(values['warm_night_mode'], 'clock');
      expect(values['warm_night_start'], -30);
      expect(values['warm_night_end'], 90);
      expect(values['warm_night_fade'], 45);
      expect(values['daylight_enabled'], true);
      expect(values['daylight_start'], 20);
      expect(values['daylight_end'], -40);
      expect(values['daylight_fade'], 100);
      expect(values['color_sensitivity'], 1.4);
    });

    test('maps duration presets from Python CSV and serialized Rust shape', () {
      expect(
        durationPresetValuesForTest({
          'duration_picker_presets': '10,30,forever',
        }),
        ['10', '30', '0'],
      );
      expect(
        durationPresetValuesForTest({
          'duration_picker_presets': {
            'values': [5, 45, 0, 10080, 0, 0, 0, 0],
            'len': 4,
          },
        }, includeValues: const [
          '240',
        ]),
        ['5', '45', '0', '10080', '240'],
      );
      expect(
        positiveDurationPresetValuesForTest(
          const ['0'],
          includeValues: const ['45', '0'],
        ),
        ['45'],
      );
      expect(
        positiveDurationPresetValuesForTest(const ['0']),
        ['5', '60', '240'],
      );
      expect(
        () => durationPresetValuesForTest({
          'duration_picker_presets': '10,broken,forever',
        }),
        throwsA(isA<FormatException>()),
      );
      expect(
        () => durationPresetValuesForTest({
          'duration_picker_presets': {'values': '10,30'},
        }),
        throwsA(isA<FormatException>()),
      );
      expect(
        () => expertDurationMinutesForTest(
          const {'default_boost_duration_minutes': 12000},
          'default_boost_duration_minutes',
          fallback: 5,
        ),
        throwsA(isA<FormatException>()),
      );
      expect(
        expertCursorStepMinutesForTest(const {'rhythm_cursor_step_min': 5}),
        5,
      );
      expect(
        () => expertCursorStepMinutesForTest(
          const {'rhythm_cursor_step_min': 0},
        ),
        throwsA(isA<FormatException>()),
      );
      expect(
        expertCardFreshnessMinutesForTest(
          const {'card_freshness_minutes': 15},
        ),
        15,
      );
      expect(
        () => expertCardFreshnessMinutesForTest(
          const {'card_freshness_minutes': 0},
        ),
        throwsA(isA<FormatException>()),
      );
      expect(
        expertBoostDefaultForTest(const {'boost_default': 45}),
        45,
      );
      expect(
        () => expertBoostDefaultForTest(const {'boost_default': 9}),
        throwsA(isA<FormatException>()),
      );
      expect(
        expertControlsPulseWindowHoursForTest(
          const {'controls_pulse_window_hours': 0.25},
        ),
        0.25,
      );
      expect(
        () => expertControlsPulseWindowHoursForTest(
          const {'controls_pulse_window_hours': 0.2},
        ),
        throwsA(isA<FormatException>()),
      );
      expect(
        expertControlsRecentWindowMinutesForTest(
          const {'controls_recent_window_minutes': 120},
        ),
        120,
      );
      expect(
        () => expertControlsRecentWindowMinutesForTest(
          const {'controls_recent_window_minutes': 121},
        ),
        throwsA(isA<FormatException>()),
      );
      final now = DateTime.utc(2026, 6, 20, 12);
      expect(
        controlPulseStateForTest(
          now.subtract(const Duration(minutes: 4)),
          now,
          6.0,
          5,
        ),
        'recent',
      );
      expect(
        controlPulseStateForTest(
          now.subtract(const Duration(hours: 3)),
          now,
          6.0,
          5,
        ),
        'pulse',
      );
      expect(
        controlPulseStateForTest(
          now.subtract(const Duration(hours: 7)),
          now,
          6.0,
          5,
        ),
        'none',
      );
      expect(nudgeExpertPhaseHourForTest(6.0, 1, 12), 6.2);
      expect(nudgeExpertPhaseHourForTest(0.0, -1, 15), 23.75);
    });
  });

  group('expert control binding mapping', () {
    test('distinguishes paused executable bindings from integration gaps', () {
      final controls = expertControlSummariesFromModernBindingsForTest(
        bindingsRaw: {
          'bindings': [
            {
              'id': 'paused-binding',
              'source_node_id': 'button-1',
              'trigger': {'kind': 'button', 'button_action': 'on_press'},
              'action': {
                'kind': 'catalog',
                'compiled': {'source_id': 'circadian_on'},
              },
              'targets': ['area-kitchen'],
              'enabled': false,
            },
            {
              'id': 'setup-binding',
              'source_node_id': 'button-2',
              'trigger': {'kind': 'button', 'button_action': 'on_press'},
              'action': {
                'kind': 'catalog',
                'compiled': {'source_id': 'circadian_on'},
              },
              'enabled': false,
            },
            {
              'id': 'future-binding',
              'source_node_id': 'button-3',
              'trigger': {'kind': 'button', 'button_action': 'on_press'},
              'action': {
                'kind': 'unsupported_imported',
                'source_action_id': 'future_action',
              },
              'targets': ['area-kitchen'],
              'enabled': false,
            },
          ],
        },
      );

      final byId = {for (final control in controls) control['id']: control};

      expect(byId['paused-binding']?['status'], 'inactive');
      expect(byId['paused-binding']?['supported'], true);
      expect(byId['paused-binding']?['inactive'], true);
      expect(byId['setup-binding']?['status'], 'not_configured');
      expect(byId['setup-binding']?['supported'], true);
      expect(byId['future-binding']?['status'], 'needs_integration');
      expect(byId['future-binding']?['supported'], false);
      expect(byId['future-binding']?['inactive'], true);
    });

    test('preserves section reach feedback area from target lists', () {
      final controls = expertControlSummariesFromModernBindingsForTest(
        bindingsRaw: {
          'bindings': [
            {
              'id': 'section-reach',
              'source_node_id': 'button-1',
              'trigger': {'kind': 'button', 'button_action': 'on_press'},
              'action': {
                'kind': 'catalog',
                'compiled': {'source_id': 'circadian_on'},
              },
              'target_lists': [
                {
                  'id': 'reach-1',
                  'targets': ['section-island'],
                  'feedback_area': 'area-kitchen',
                },
              ],
              'enabled': true,
            },
          ],
        },
        scopeRaw: {
          'zones': [
            {
              'id': 'zone-main',
              'name': 'Main',
              'areas': [
                {
                  'id': 'area-kitchen',
                  'name': 'Kitchen',
                  'sections': [
                    {'id': 'section-island', 'name': 'Island'},
                  ],
                },
              ],
            },
          ],
        },
      );

      expect(controls.single['scopes'], [
        {
          'area_ids': <String>[],
          'section_ids': ['section-island'],
          'feedback_area': 'area-kitchen',
          'mode': 'on_off',
        },
      ]);
    });

    test('maps removed-project aliases to first-class Expert target grains', () {
      for (final actionId in [
        'glo_up',
        'full_send',
        'glozone_down',
        'glozone_reset',
        'glozone_reset_full',
        'glozone_reset_frozen',
        'sun_up',
        'sun_down',
      ]) {
        final payload = expertInputBindingActionPayloadForTest(
          actionId: actionId,
          sectionIds: const ['section-island'],
        );
        expect(payload['target_grain'], 'zone', reason: actionId);
      }

      for (final actionId in ['set_wake', 'set_bed', 'set_wake_or_bed']) {
        final payload = expertInputBindingActionPayloadForTest(
          actionId: actionId,
          areaIds: const ['area-kitchen'],
        );
        expect(payload['target_grain'], 'room', reason: actionId);
      }

      final momentPayload = expertInputBindingActionPayloadForTest(
        actionId: 'set_movie_time',
        areaIds: const ['area-kitchen'],
      );
      expect(momentPayload['target_grain'], 'global');
    });

    test('uses catalog allowed values for switchmap if-off options', () {
      final summary = expertSwitchmapLoadSummaryForTest(
        actionsRaw: {
          'actions': [
            {
              'python_action_id': 'bright_up',
              'category': 'Adjust areas in Reach',
              'label': 'Bright Up',
              'supports_when_off': true,
              'allowed_when_off_values': [
                'set_nitelite',
                'set_britelite',
                'glo_reset',
              ],
            },
            {
              'python_action_id': 'set_nitelite',
              'category': 'Adjust areas in Reach',
              'label': 'NiteLite',
              'supports_when_off': false,
              'allowed_when_off_values': <String>[],
            },
            {
              'python_action_id': 'set_britelite',
              'category': 'Adjust areas in Reach',
              'label': 'BriteLite',
              'supports_when_off': false,
              'allowed_when_off_values': <String>[],
            },
            {
              'python_action_id': 'glo_reset',
              'category': 'Adjust areas in Reach',
              'label': 'Reset',
              'supports_when_off': false,
              'allowed_when_off_values': <String>[],
            },
          ],
        },
      );

      expect(summary['when_off_options'], [
        'set_nitelite',
        'set_britelite',
        'glo_reset',
      ]);
      expect(
        summary['when_off_labels'],
        containsPair('set_nitelite', 'NiteLite'),
      );
      expect(
        summary['when_off_options'],
        isNot(contains('bright_up')),
      );
    });

    test('loads runtime switchmap effective and custom mappings', () {
      final summary = expertSwitchmapLoadSummaryForTest(
        actionsRaw: {
          'actions': [
            {
              'python_action_id': 'circadian_on',
              'category': 'Power',
              'label': 'On',
              'supports_when_off': false,
              'allowed_when_off_values': <String>[],
            },
            {
              'python_action_id': 'set_nitelite',
              'category': 'Preset',
              'label': 'NiteLite',
              'supports_when_off': false,
              'allowed_when_off_values': <String>[],
            },
          ],
        },
        switchmapRaw: {
          'mappings': {
            'button': {
              'name': 'Button',
              'buttons': ['on', 'off'],
              'action_types': ['short_release'],
              'default_mapping': {
                'on_short_release': 'circadian_on',
              },
              'effective_mapping': {
                'on_short_release': 'set_nitelite',
              },
              'has_custom': true,
            },
          },
          'custom_mappings': {
            'button': {
              'on_short_release': 'set_nitelite',
            },
          },
        },
      );

      expect(summary['type_ids'], contains('button'));
      expect(
        summary['button_effective_mapping'],
        containsPair('on_short_release', 'set_nitelite'),
      );
      expect(
        summary['button_custom_mapping'],
        containsPair('on_short_release', 'set_nitelite'),
      );
    });

    test('builds scope action requests against Expert endpoints', () {
      final areaRequest = expertAreaActionRequestForTest(
        areaId: 'area kitchen/1',
        action: 'set_color_temperature',
        extra: const {'kelvin': 3333},
      );
      expect(
        areaRequest['path'],
        'api/light-runtimes/removed-circadian/areas/area%20kitchen%2F1/action',
      );
      expect(
        expertAreaNowPathForTest('area kitchen/1'),
        'api/light-runtimes/removed-circadian/areas/area%20kitchen%2F1/now',
      );
      expect(areaRequest['data'], {
        'action': 'set_color_temperature',
        'kelvin': 3333,
      });
      expect(
        expertAreaActionRequestForTest(
          areaId: 'area-kitchen',
          action: 'boost_on',
          extra: const {
            'duration_minutes': 15,
            'boost_brightness': 45,
          },
        )['data'],
        {
          'action': 'boost_on',
          'duration_minutes': 15,
          'boost_brightness': 45,
        },
      );
      expect(
        expertAreaActionRequestForTest(
          areaId: 'area-kitchen',
          action: 'set_brightness',
          extra: const {'brightness': 67},
        )['data'],
        {'action': 'set_brightness', 'brightness': 67},
      );
      expect(
        expertAreaActionRequestForTest(
          areaId: 'area-kitchen',
          action: 'set_circadian_enabled',
          extra: const {'enabled': false},
        )['data'],
        {'action': 'set_circadian_enabled', 'enabled': false},
      );

      final sectionRequest = expertSectionActionRequestForTest(
        sectionId: 'section island/1',
        action: 'set_auto_off',
        extra: const {'duration_minutes': 15},
      );

      expect(
        sectionRequest['path'],
        'api/light-runtimes/removed-circadian/sections/section%20island%2F1/action',
      );
      expect(sectionRequest['data'], {
        'action': 'set_auto_off',
        'duration_minutes': 15,
      });

      expect(
        expertAreaLayerRequestForTest(
          areaId: 'area kitchen/1',
          data: const {
            'fields': {'balance': 0.75},
          },
        ),
        {
          'path':
              'api/light-runtimes/removed-circadian/areas/area%20kitchen%2F1/layer',
          'data': {
            'fields': {'balance': 0.75},
          },
        },
      );
      expect(
        expertAreaLightFilterRequestForTest(
          areaId: 'area kitchen/1',
          deviceId: 'device light/1',
          purpose: 'Accent',
        ),
        {
          'path':
              'api/light-runtimes/removed-circadian/areas/area%20kitchen%2F1/light-filter',
          'data': {
            'device_id': 'device light/1',
            'preset_id': 'Accent',
          },
        },
      );
      expect(
        expertSectionLayerRequestForTest(
          sectionId: 'section island/1',
          data: const {
            'fields': {
              'brightness_trim': {'value': -12.0},
            },
          },
        ),
        {
          'path':
              'api/light-runtimes/removed-circadian/sections/section%20island%2F1/layer',
          'data': {
            'fields': {
              'brightness_trim': {'value': -12.0},
            },
          },
        },
      );
    });
  });

  group('expert rhythm profile mapping', () {
    test('loads runtime rhythm preset profiles without app profile fallback',
        () {
      final summary = expertRhythmPresetLoadSummaryForTest({
        'profiles': [
          {
            'id': 'young',
            'curve': {
              'schedule': {
                'wake': {'hour': 6.0},
                'bed': {'hour': 18.0},
              },
              'ascend_start': 2.0,
              'descend_start': 13.0,
            },
          },
          {
            'id': 'adult',
            'curve': {
              'schedule': {
                'wake': {'hour': 7.0},
                'bed': {'hour': 21.0},
              },
              'ascend_start': 3.0,
              'descend_start': 13.0,
            },
          },
          {
            'id': 'custom',
            'curve': {
              'schedule': {
                'wake': {'hour': 6.0},
                'bed': {'hour': 22.0},
              },
              'ascend_start': 3.0,
              'descend_start': 12.0,
            },
          },
        ],
      });

      expect(summary['path'],
          'api/light-runtimes/removed-circadian/rhythm-presets');
      expect(summary['names'], ['young', 'adult', 'custom']);
      expect(summary['adult_wake'], 7.0);
      expect(summary['adult_bed'], 21.0);
      expect(summary['adult_ascend'], 3.0);
      expect(summary['adult_descend'], 13.0);
      expect(summary['young_ascend'], 2.0);
      expect(summary['young_descend'], 13.0);
    });

    test('builds runtime default profile update requests', () {
      final request = expertProfileUpdateRequestForTest(
        {
          'id': 'Main',
          'name': 'Default zone',
          'max_dim_steps': 8,
          'unknown_runtime_field': {'preserve': true},
        },
        {
          'id': 'ignored update id',
          'min_brightness': 10,
          'max_brightness': 90,
          'curve': {
            'schedule': {
              'wake': {'hour': 6.5},
              'bed': {'hour': 22.25},
            },
          },
        },
      );
      final data = request['data']! as Map<String, Object?>;

      expect(
        request['path'],
        'api/light-runtimes/removed-circadian/profile',
      );
      expect(data['id'], 'Main');
      expect(data['name'], 'Default zone');
      expect(data['max_dim_steps'], 8);
      expect(data['unknown_runtime_field'], {'preserve': true});
      expect(data['min_brightness'], 10);
      expect(data['max_brightness'], 90);
      expect(
        ((data['curve']! as Map<dynamic, dynamic>)['schedule']
            as Map<dynamic, dynamic>)['wake'],
        {'hour': 6.5},
      );
    });

    test('parses runtime preview payloads only when required fields exist', () {
      final summary = expertRuntimePreviewSummaryForTest(
        sunTimes: {
          'sunrise_hour': 6.25,
          'sunset_hour': 18.5,
          'noon_hour': 12.4,
          'midnight_hour': 0.4,
        },
        curve: {
          'hours': [0.0, 12.0, 24.0],
          'brightness': [20, 80, 20],
          'kelvin': [2200, 5200, 2200],
        },
        steps: {
          'step_up': {
            'steps': [
              {'hour': 7.0, 'brightness': 30, 'kelvin': 2400},
            ],
          },
          'step_down': {
            'steps': [
              {'hour': 22.0, 'brightness': 15, 'kelvin': 2100},
            ],
          },
        },
      );

      expect(summary['sunrise'], 6.25);
      expect(summary['solar_noon'], 12.4);
      expect(summary['curve_count'], 3);
      expect(summary['first_curve_hour'], 0.0);
      expect(summary['first_curve_brightness'], 20);
      expect(summary['step_up_count'], 1);
      expect(summary['step_down_count'], 1);
      expect(summary['first_step_up_hour'], 7.0);
    });

    test('throws on malformed runtime preview payloads', () {
      expect(
        () => expertRuntimePreviewSummaryForTest(
          sunTimes: {
            'sunrise_hour': 6.25,
            'sunset_hour': 18.5,
            'midnight_hour': 0.4,
          },
          curve: {
            'hours': [0.0],
            'brightness': [20],
            'kelvin': [2200],
          },
          steps: {
            'step_up': {
              'steps': [
                {'hour': 7.0, 'brightness': 30, 'kelvin': 2400},
              ],
            },
            'step_down': {
              'steps': [
                {'hour': 22.0, 'brightness': 15, 'kelvin': 2100},
              ],
            },
          },
        ),
        throwsA(isA<FormatException>()),
      );
      expect(
        () => expertRuntimePreviewSummaryForTest(
          sunTimes: {
            'sunrise_hour': 6.25,
            'sunset_hour': 18.5,
            'noon_hour': 12.4,
            'midnight_hour': 0.4,
          },
          curve: {
            'hours': [0.0, 12.0],
            'brightness': [20],
            'kelvin': [2200, 5200],
          },
          steps: {
            'step_up': {
              'steps': [
                {'hour': 7.0, 'brightness': 30, 'kelvin': 2400},
              ],
            },
            'step_down': {
              'steps': [
                {'hour': 22.0, 'brightness': 15, 'kelvin': 2100},
              ],
            },
          },
        ),
        throwsA(isA<FormatException>()),
      );
      expect(
        () => expertRuntimePreviewSummaryForTest(
          sunTimes: {
            'sunrise_hour': 6.25,
            'sunset_hour': 18.5,
            'noon_hour': 12.4,
            'midnight_hour': 0.4,
          },
          curve: {
            'hours': [0.0],
            'brightness': [20],
            'kelvin': [2200],
          },
          steps: {
            'step_up': {
              'steps': [
                {'hour': 7.0, 'brightness': 30},
              ],
            },
            'step_down': {
              'steps': [
                {'hour': 22.0, 'brightness': 15, 'kelvin': 2100},
              ],
            },
          },
        ),
        throwsA(isA<FormatException>()),
      );
    });

    test('throws on malformed runtime rhythm profile payloads', () {
      expect(
        () => expertRhythmPresetLoadSummaryForTest({
          'profiles': [
            {
              'id': 'adult',
              'curve': {
                'schedule': {
                  'wake': {'hour': 7.0},
                  'bed': {'hour': 21.0},
                },
                'ascend_start': 3.0,
              },
            },
          ],
        }),
        throwsA(isA<FormatException>()),
      );

      expect(
        () => expertDefinitionValuesFromProfileForTest({
          'id': 'Main',
          'min_brightness': 10,
          'max_brightness': 90,
          'min_color_temp': 2000,
          'curve': {
            'schedule': {
              'wake': {'hour': 6.5},
              'bed': {'hour': 22.25},
            },
            'ascend_start': 3.5,
            'wake_speed': 9,
            'bed_speed': 9,
            'bed_brightness': 25,
          },
        }),
        throwsA(isA<FormatException>()),
      );

      expect(
        () => expertDefinitionValuesFromProfileForTest({
          'id': 'Main',
          'min_brightness': 90,
          'max_brightness': 10,
          'min_color_temp': 2000,
          'max_color_temp': 6200,
          'curve': {
            'schedule': {
              'wake': {'hour': 6.5},
              'bed': {'hour': 22.25},
            },
            'ascend_start': 3.5,
            'wake_speed': 9,
            'bed_speed': 9,
            'bed_brightness': 25,
          },
        }),
        throwsA(isA<FormatException>()),
      );
    });

    test('builds an expert sigmoid profile payload from editor values', () {
      final config = expertProfileConfigFromDefinitionForTest(
        wakeHour: 6.5,
        bedHour: 22.25,
        minBrightness: 12,
        maxBrightness: 90,
        sleepBrightness: 24,
        minKelvin: 2100,
        maxKelvin: 6100,
        transitionMinutes: 30,
        phaseBalance: 0.5,
        daylightEnabled: false,
        daylightCct: 5400,
        daylightStart: 30,
        daylightEnd: -120,
        daylightFade: 90,
        colorSensitivity: 1.25,
        brightnessSensitivityEnabled: false,
        brightnessSensitivity: 1.5,
        warmNightEnabled: true,
        warmNightMode: 'window',
        warmNightTarget: 2400,
        warmNightStart: -90,
        warmNightEnd: 30,
        warmNightFade: 150,
        base: {
          'id': 'rhythm',
          'name': 'Saved name should be replaced',
          'max_dim_steps': 9,
          'step_fallback_minutes': 45,
        },
      );
      final curve = config['curve'] as Map<dynamic, dynamic>;
      final schedule = curve['schedule'] as Map<dynamic, dynamic>;

      expect(config['id'], 'expert');
      expect(config['name'], 'Saved name should be replaced');
      expect(config['min_brightness'], 12);
      expect(config['max_brightness'], 90);
      expect(config['min_color_temp'], 2100);
      expect(config['max_color_temp'], 6100);
      expect(config['max_dim_steps'], 9);
      expect(config['step_fallback_minutes'], 45);
      expect(config['daylight_enabled'], false);
      expect(config['daylight_cct'], 5400);
      expect(config['daylight_start'], 30);
      expect(config['daylight_end'], -120);
      expect(config['daylight_fade'], 90);
      expect(config['color_sensitivity'], 1.25);
      expect(config['brightness_sensitivity_enabled'], false);
      expect(config['brightness_sensitivity'], 1.5);
      expect(config['warm_night_enabled'], true);
      expect(config['warm_night_mode'], 'window');
      expect(config['warm_night_target'], 2400);
      expect(config['warm_night_start'], -90);
      expect(config['warm_night_end'], 30);
      expect(config['warm_night_fade'], 150);
      expect(curve['type'], 'sigmoid');
      expect((schedule['wake'] as Map<dynamic, dynamic>)['hour'], 6.5);
      expect((schedule['bed'] as Map<dynamic, dynamic>)['hour'], 22.25);
      expect(curve['ascend_start'], 3.5);
      expect(curve['descend_start'], 18.25);
      expect(curve['wake_speed'], 9);
      expect(curve['bed_speed'], 9);
      expect(curve['wake_brightness'], 50);
      expect(curve['bed_brightness'], 15);
    });

    test('preserves low removed-project profile color temperatures', () {
      final config = expertProfileConfigFromDefinitionForTest(
        wakeHour: 6.5,
        bedHour: 22.25,
        minBrightness: 12,
        maxBrightness: 90,
        sleepBrightness: 24,
        minKelvin: 500,
        maxKelvin: 7600,
        transitionMinutes: 30,
        phaseBalance: 0.5,
      );

      expect(config['min_color_temp'], 500);
      expect(config['max_color_temp'], 7600);

      final values = expertDefinitionValuesFromProfileForTest({
        'id': 'expert',
        'min_brightness': 10,
        'max_brightness': 90,
        'min_color_temp': 500,
        'max_color_temp': 7600,
        ..._runtimeProfileSkyFields(),
        'curve': {
          'type': 'sigmoid',
          'schedule': {
            'wake': {'hour': 6.5},
            'bed': {'hour': 22.25},
            'alternate_days': <dynamic>[],
          },
          'ascend_start': 3.5,
          'descend_start': 18.25,
          'wake_speed': 9,
          'bed_speed': 9,
          'wake_brightness': 50,
          'bed_brightness': 25,
        },
      });

      expect(values['min_kelvin'], 500);
      expect(values['max_kelvin'], 7600);
    });

    test('loads editor values from an expert sigmoid profile payload', () {
      final values = expertDefinitionValuesFromProfileForTest({
        'id': 'expert',
        'min_brightness': 10,
        'max_brightness': 90,
        'min_color_temp': 2000,
        'max_color_temp': 6200,
        ..._runtimeProfileSkyFields(
          daylightEnabled: false,
          daylightCct: 5400,
          daylightStart: 30,
          daylightEnd: -120,
          daylightFade: 90,
          colorSensitivity: 1.25,
          brightnessSensitivityEnabled: false,
          brightnessSensitivity: 1.5,
          warmNightEnabled: true,
          warmNightMode: 'window',
          warmNightTarget: 2400,
          warmNightStart: -90,
          warmNightEnd: 30,
          warmNightFade: 150,
        ),
        'curve': {
          'type': 'sigmoid',
          'schedule': {
            'wake': {'hour': 6.5},
            'bed': {'hour': 22.25},
            'alternate_days': <dynamic>[],
          },
          'ascend_start': 3.5,
          'descend_start': 18.25,
          'wake_speed': 9,
          'bed_speed': 9,
          'wake_brightness': 50,
          'bed_brightness': 25,
        },
      });

      expect(values['sleep_pattern'], 'expert');
      expect(values['wake_hour'], 6.5);
      expect(values['bed_hour'], 22.25);
      expect(values['min_brightness'], 10);
      expect(values['max_brightness'], 90);
      expect(values['sleep_brightness'], 30);
      expect(values['min_kelvin'], 2000);
      expect(values['max_kelvin'], 6200);
      expect(values['transition_minutes'], 28.18);
      expect(values['phase_balance'], 0.5);
      expect(values['daylight_enabled'], false);
      expect(values['daylight_cct'], 5400);
      expect(values['daylight_start'], 30);
      expect(values['daylight_end'], -120);
      expect(values['daylight_fade'], 90);
      expect(values['color_sensitivity'], 1.25);
      expect(values['brightness_sensitivity_enabled'], false);
      expect(values['brightness_sensitivity'], 1.5);
      expect(values['warm_night_enabled'], true);
      expect(values['warm_night_mode'], 'window');
      expect(values['warm_night_target'], 2400);
      expect(values['warm_night_start'], -90);
      expect(values['warm_night_end'], 30);
      expect(values['warm_night_fade'], 150);
    });
  });

  group('expert moment scene mapping', () {
    test('builds modern scene payload with expert metadata and room targets',
        () {
      final scene = modernSceneForExpertMomentForTest(
        id: 'evening',
        name: 'Evening',
        icon: 'mdi:weather-night',
        category: 'fun',
        defaultAction: 'nitelite',
        timerSeconds: 900,
        targetIds: ['living-room', 'kitchen'],
        exceptions: {
          'kitchen': {'action': 'leave_alone', 'timer': 0},
        },
      );
      final extensions = scene['extensions'] as Map<dynamic, dynamic>;
      final metadata = extensions['circadian_expert'] as Map<dynamic, dynamic>;
      final light = scene['light'] as Map<dynamic, dynamic>;
      final entries = light['entries'] as List<dynamic>;

      expect(scene['id'], 'evening');
      expect(scene['name'], 'Evening');
      expect(metadata['icon'], 'mdi:weather-night');
      expect(metadata['category'], 'fun');
      expect(metadata['default_action'], 'nitelite');
      expect(metadata['timer'], 900);
      expect((metadata['exceptions'] as Map<dynamic, dynamic>)['kitchen'],
          {'action': 'leave_alone', 'timer': 0});
      expect(entries, hasLength(2));
      expect((entries.first as Map<dynamic, dynamic>)['target'],
          {'kind': 'node', 'node_id': 'kitchen'});
      expect((entries.first as Map<dynamic, dynamic>)['value'],
          {'kind': 'preset', 'preset': 'nitelite'});
    });

    test('maps fallback moment actions to Expert area actions', () {
      expect(expertAreaActionForMomentActionForTest('lights_on'), 'lights_on');
      expect(
        expertAreaActionForMomentActionForTest('nitelite'),
        'set_nitelite',
      );
      expect(
        expertAreaActionForMomentActionForTest('britelite'),
        'set_britelite',
      );
      expect(
        expertAreaActionForMomentActionForTest('wake_or_bed'),
        'set_wake_or_bed',
      );
      expect(expertAreaActionForMomentActionForTest('reset'), 'glo_reset');
      expect(expertAreaActionForMomentActionForTest('leave_alone'), isNull);
    });

    test('loads expert moment fields from modern scene metadata', () {
      final values = expertMomentValuesFromModernSceneForTest({
        'id': 'evening',
        'name': 'Evening',
        'source': {'kind': 'user'},
        'extensions': {
          'circadian_expert': {
            'icon': 'mdi:weather-night',
            'category': 'fun',
            'default_action': 'off',
            'timer': 600,
            'exceptions': {
              'kitchen': {'action': 'leave_alone', 'timer': 0},
            },
          },
        },
        'light': {
          'entries': [
            {
              'target': {'kind': 'node', 'node_id': 'kitchen'},
              'value': {'kind': 'preset', 'preset': 'off'},
            },
          ],
        },
      });

      expect(values['id'], 'evening');
      expect(values['name'], 'Evening');
      expect(values['icon'], 'mdi:weather-night');
      expect(values['category'], 'fun');
      expect(values['default_action'], 'off');
      expect(values['timer'], 600);
      expect(values['exceptions'], {
        'kitchen': {'action': 'leave_alone', 'timer': 0},
      });
    });
  });
}

Map<String, Object?> _runtimeProfileSkyFields({
  bool daylightEnabled = true,
  int daylightCct = 5000,
  int daylightStart = 60,
  int daylightEnd = -60,
  int daylightFade = 60,
  double colorSensitivity = 1,
  bool brightnessSensitivityEnabled = true,
  double brightnessSensitivity = 1,
  bool warmNightEnabled = true,
  String warmNightMode = 'all',
  int warmNightTarget = 2300,
  int warmNightStart = -60,
  int warmNightEnd = 60,
  int warmNightFade = 120,
}) {
  return {
    'daylight_enabled': daylightEnabled,
    'daylight_cct': daylightCct,
    'daylight_start': daylightStart,
    'daylight_end': daylightEnd,
    'daylight_fade': daylightFade,
    'color_sensitivity': colorSensitivity,
    'brightness_sensitivity_enabled': brightnessSensitivityEnabled,
    'brightness_sensitivity': brightnessSensitivity,
    'warm_night_enabled': warmNightEnabled,
    'warm_night_mode': warmNightMode,
    'warm_night_target': warmNightTarget,
    'warm_night_start': warmNightStart,
    'warm_night_end': warmNightEnd,
    'warm_night_fade': warmNightFade,
  };
}
