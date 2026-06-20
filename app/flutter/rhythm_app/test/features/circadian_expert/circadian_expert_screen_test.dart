import 'package:flutter_test/flutter_test.dart';
import 'package:rhythm_app/features/circadian_expert/circadian_expert_screen.dart';

void main() {
  group('expert settings server mapping', () {
    test('maps modern solar settings into legacy expert UI keys', () {
      final values = expertSettingsValuesFromServerForTest({
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
        'controls_pulse_window_hours': 9,
        'controls_recent_window_minutes': 7,
        'activity_log_min_entries': 150,
        'activity_log_min_days': 14,
        'activity_log_flush_interval_minutes': 30,
        'tick_repeat_after_user_action': 12,
        'tick_repeat_after_autonomous_change': 4,
        'periodic_refresh_interval_minutes': 45,
        'ct_compensation': {
          'enabled': true,
          'begin_kelvin': 1600,
          'end_kelvin': 2300,
          'factor': 1.6,
        },
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
      });

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
      expect(values['controls_pulse_window_hours'], 9);
      expect(values['controls_recent_window_minutes'], 7);
      expect(values['activity_log_min_entries'], 150);
      expect(values['activity_log_min_days'], 14);
      expect(values['activity_log_flush_interval_minutes'], 30);
      expect(values['tick_repeat_after_user_action'], 12);
      expect(values['tick_repeat_after_autonomous_change'], 4);
      expect(values['periodic_refresh_interval_minutes'], 45);
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

    test('maps legacy expert UI keys to modern server keys', () {
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
      expectEntry('daylight_start', 15, 'daylight_start', 15);
    });

    test('maps duration presets from legacy CSV and serialized Rust shape', () {
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
        expertDurationMinutesForTest(
          const {'default_boost_duration_minutes': 12000},
          'default_boost_duration_minutes',
          fallback: 60,
        ),
        10080,
      );
      expect(
        expertCursorStepMinutesForTest(const {'rhythm_cursor_step_min': 0}),
        1,
      );
      expect(
        expertCursorStepMinutesForTest(const {'rhythm_cursor_step_min': 90}),
        60,
      );
      expect(
        expertControlsPulseWindowHoursForTest(
          const {'controls_pulse_window_hours': 0},
        ),
        1,
      );
      expect(
        expertControlsPulseWindowHoursForTest(
          const {'controls_pulse_window_hours': 200},
        ),
        168,
      );
      expect(
        expertControlsRecentWindowMinutesForTest(
          const {'controls_recent_window_minutes': 0},
        ),
        1,
      );
      expect(
        expertControlsRecentWindowMinutesForTest(
          const {'controls_recent_window_minutes': 2000},
        ),
        1440,
      );
      final now = DateTime.utc(2026, 6, 20, 12);
      expect(
        controlPulseStateForTest(
          now.subtract(const Duration(minutes: 4)),
          now,
          6,
          5,
        ),
        'recent',
      );
      expect(
        controlPulseStateForTest(
          now.subtract(const Duration(hours: 3)),
          now,
          6,
          5,
        ),
        'pulse',
      );
      expect(
        controlPulseStateForTest(
          now.subtract(const Duration(hours: 7)),
          now,
          6,
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
        )['data'],
        {'action': 'boost_on'},
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

    test('loads editor values from an expert sigmoid profile payload', () {
      final values = expertDefinitionValuesFromProfileForTest({
        'id': 'expert',
        'min_brightness': 10,
        'max_brightness': 90,
        'min_color_temp': 2000,
        'max_color_temp': 6200,
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
