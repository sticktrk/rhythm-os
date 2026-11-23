import 'dart:convert';
import 'package:crypto/crypto.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'customer_assistant_service.dart';
import 'home_agent_client.dart';
import 'lighting_chat_service.dart';

class AgentTarget {
  AgentTarget(this.ref, this.id, this.name, this.kind, this.tools);
  final String ref, id, name, kind;
  final Set<String> tools;
}

class AgentReview {
  AgentReview(this.target, this.operation, this.arguments, this.description,
      this.proof, this.digest);
  final AgentTarget target;
  final String operation, description, digest;
  final CustomerAction arguments, proof;
}

class HomeAgentCatalog {
  HomeAgentCatalog(this.hub, this.targets);
  final LightingChatHub hub;
  final Map<String, AgentTarget> targets;

  static Future<HomeAgentCatalog> load(
      LightingChatHub hub, HomeAgentClient client) async {
    final targets = <String, AgentTarget>{};
    void add(String kind, String id, String name, Set<String> tools) {
      if (id.isEmpty) return;
      final ref = client.reference(kind, id);
      targets[ref] =
          AgentTarget(ref, id, name.isEmpty ? id : name, kind, tools);
    }

    for (final r in hub.rooms) {
      add('r', r.id, r.name, hub.tools);
    }
    for (final n
        in hub.nodes.where((n) => n.kind == RhythmNodeKind.motionSensor)) {
      add(
          'm',
          n.id,
          n.name,
          hub.catalog
              .intersection({'start_motion_walk', 'plan_motion_target'}));
    }
    if (hub.catalog.contains('read_automation_settings')) {
      final config = await hub.call('read_automation_settings', {});
      for (final (field, kind, op) in [
        ('scenes', 'c', 'plan_home_scene_edit'),
        ('schedules', 's', 'plan_light_schedule_edit')
      ]) {
        for (final row in (config[field] as List? ?? []).whereType<Map>()) {
          if (row['id'] is String &&
              row['name'] is String &&
              hub.catalog.contains(op)) {
            add(kind, row['id'], row['name'], {op});
          }
        }
      }
    }
    return HomeAgentCatalog(hub, targets);
  }

  /// Longest names first so "main bedroom" cannot also match "bedroom".
  String redact(String text, [String? selected]) {
    final groups = <String, List<AgentTarget>>{};
    for (final t in targets.values) {
      groups.putIfAbsent(t.name.toLowerCase(), () => []).add(t);
    }
    final names = groups.keys.toList()
      ..sort((a, b) => b.length.compareTo(a.length));
    if (names.isEmpty) return text;
    final pattern = RegExp(
        r'(?<![\p{L}\p{N}])(' +
            names.map(RegExp.escape).join('|') +
            r')(?![\p{L}\p{N}])',
        caseSensitive: false,
        unicode: true);
    return text.replaceAllMapped(pattern, (m) {
      final matches = groups[m[0]!.toLowerCase()]!;
      return matches.length == 1
          ? matches.single.ref
          : matches.where((m) => m.ref == selected).firstOrNull?.ref ??
              '[ambiguous target]';
    });
  }

  Future<List<CustomerAction>> context(String prompt, String? selected,
      [Map<String, dynamic>? motion]) async {
    final mentioned =
        targets.values.where((t) => prompt.contains(t.ref)).toList();
    final whole = RegExp(
            r'\b(all rooms|every room|whole (house|home)|throughout the house)\b',
            caseSensitive: false)
        .hasMatch(prompt);
    const refPattern = r'[rscm]_[a-f0-9]{24}';
    final exclusions = RegExp(
        r'\b(?:except(?: for)?|exclude|excluding|skip|leave(?: out)?)\s+((?:(?:the\s+)?' +
            refPattern +
            r')(?:(?:\s*,\s*(?:and\s+)?|\s+(?:and|or)\s+)(?:the\s+)?' +
            refPattern +
            r')*)',
        caseSensitive: false);
    final excluded = {
      for (final group in exclusions.allMatches(prompt))
        for (final match in RegExp(refPattern).allMatches(group[1]!)) match[0]
    };
    final candidates = {
      ...mentioned,
      ...targets.values.where((t) =>
          t.ref == selected ||
          t.ref == motion?['sensor'] ||
          t.ref == motion?['room']),
      ...targets.values
    }.take(48);
    final result = <CustomerAction>[];
    for (final t in candidates) {
      final inScope = !excluded.contains(t.ref) &&
          !prompt.contains('[ambiguous target]') &&
          (mentioned.contains(t) ||
              (mentioned.isEmpty && t.ref == selected) ||
              (motion != null &&
                  (mentioned.isEmpty ||
                      mentioned.any((m) =>
                          m.ref == motion['room'] ||
                          m.ref == motion['sensor'])) &&
                  (t.ref == motion['room'] || t.ref == motion['sensor'])) ||
              (whole && t.kind == 'r'));
      var tools = inScope ? t.tools.toList() : <String>[];
      if (motion?['missing_target'] != true || motion?['sensor'] != t.ref) {
        tools.remove('plan_motion_target');
      }
      if (!whole || excluded.isNotEmpty) tools.remove('plan_home_scene_edit');
      CustomerAction reading = {};
      if (t.kind == 'r' &&
          inScope &&
          result.where((r) => r.containsKey('reading')).length < 8) {
        final data = await hub.call('explain_room_output', {'room_id': t.id});
        if (data['room_id'] != t.id) throw StateError('Room changed');
        reading = lightingReading(data);
        final lights =
            (data['light_readback'] as List? ?? []).whereType<Map>().toList();
        if (lights.isNotEmpty &&
            lights.every((l) => l['control_scope'] == 'outside_ha_selection')) {
          tools = [];
        }
      }
      result.add({
        'ref': t.ref,
        'included': inScope,
        'tools': tools,
        if (reading.isNotEmpty) 'reading': reading
      });
    }
    return result;
  }

  String nameFor(String value) {
    for (final target in targets.values) {
      if (target.id == value) return target.name;
    }
    return value;
  }

  Future<AgentReview> review(
      CustomerAction step, String instance, String journey) async {
    final target = targets[step['target']], op = step['operation'] as String;
    if (target == null || !target.tools.contains(op)) {
      throw StateError('Target or capability changed');
    }
    final args = CustomerAction.from(step['arguments'] as Map);
    CustomerAction proof = {};
    late CustomerAction dispatch;
    late String description, operation;
    if (LightingProposal.supported.contains(op)) {
      final p = LightingProposal.parse(
          {'operation': op, 'arguments': args}, target.tools);
      final current =
          await hub.call('explain_room_output', {'room_id': target.id});
      if (current['room_id'] != target.id) throw StateError('Room changed');
      final lights =
          (current['light_readback'] as List? ?? []).whereType<Map>().toList();
      if (lights.isNotEmpty &&
          lights.every((l) => l['control_scope'] == 'outside_ha_selection')) {
        throw StateError('Room is managed externally');
      }
      operation = op;
      dispatch = {'room_id': target.id, ...p.arguments};
      description = '${target.name} · ${p.label}. ${p.detail}';
      proof = {
        'targets': [
          for (final light in lights)
            if (light['light_id'] is String)
              jsonEncode([light['light_id'], light['control_scope']])
        ]..sort()
      };
    } else if (op == 'start_motion_walk') {
      final room = targets[args['room']];
      if (room == null || room.kind != 'r') {
        throw StateError('Choose the intended room');
      }
      operation = op;
      dispatch = {'sensor_id': target.id, 'room_id': room.id};
      description =
          'Start a 60-second walk past ${target.name} for ${room.name}.';
    } else {
      final params = <String, dynamic>{};
      if (op == 'plan_home_scene_edit') {
        params.addAll({'scene_id': target.id, 'target_mode': 'rooms'});
      } else if (op == 'plan_light_schedule_edit') {
        final live = await hub.call('read_automation_settings', {});
        final def = (live['schedules'] as List)
            .whereType<Map>()
            .singleWhere((v) => v['id'] == target.id);
        params.addAll({
          'schedule_id': target.id,
          'definition': shiftSchedule(
              CustomerAction.from(def),
              args['delta_minutes'] as int,
              args['transition'] as String? ?? 'only')
        });
      } else if (op == 'plan_motion_target') {
        final room = targets[args['room']];
        if (room == null || room.kind != 'r') {
          throw StateError('Choose the intended room');
        }
        params.addAll({
          'sensor_id': target.id,
          'room_id': room.id,
          'correlation_id': journey
        });
      } else {
        throw StateError('Unsupported workflow');
      }
      final preview = await hub.call(op, params);
      final plan = preview['plan_id'];
      if (plan is! String || preview['server_instance_id'] != instance) {
        throw StateError('Fresh preview required');
      }
      operation = op.replaceFirst('plan_', 'apply_');
      if (!hub.catalog.contains(operation)) throw StateError('Update the hub');
      dispatch = {
        'plan_id': plan,
        'confirmed': true,
        if (op == 'plan_motion_target') 'server_instance_id': instance
      };
      proof = {
        for (final k in [
          'before',
          'proposal',
          'affected_rooms',
          'before_targets',
          'after_targets',
          'sensor_id',
          'room_id'
        ])
          if (preview.containsKey(k)) k: preview[k]
      };
      final affected = (preview['affected_rooms'] as List? ?? [])
          .whereType<String>()
          .map(nameFor)
          .join(', ');
      description = switch (op) {
        'plan_home_scene_edit' =>
          'Apply ${target.name} throughout this installation. Replaces saved Mood bindings. Affected rooms: $affected.',
        'plan_light_schedule_edit' =>
          'Shift ${args['transition'] == 'all' ? 'every enabled timed transition' : 'the selected timed transition'} in ${target.name} by ${args['delta_minutes']} minutes. Affected rooms: $affected.',
        _ =>
          'Repair ${target.name} motion routing to ${targets[args['room']]!.name}.'
      };
    }
    final digest = sha256
        .convert(
            utf8.encode(jsonEncode([instance, target.ref, op, args, proof])))
        .toString();
    return AgentReview(target, operation, dispatch, description, proof, digest);
  }
}

CustomerAction shiftSchedule(CustomerAction definition, int delta,
    [String transition = 'all']) {
  if (delta == 0 || delta.abs() > 720) {
    throw const FormatException('Invalid schedule offset');
  }
  final next = jsonDecode(jsonEncode(definition)) as CustomerAction;
  final timed = (next['transitions'] as List)
      .whereType<Map>()
      .where((tr) =>
          tr['trigger_enabled'] != false &&
          tr['trigger'] is Map &&
          const {'solar', 'scheduled'}.contains(tr['trigger']['kind']) &&
          (transition == 'all' ||
              transition == 'only' ||
              tr['to_mode'] == transition))
      .toList();
  if (transition != 'all' && timed.length != 1) {
    throw const FormatException(
        'Choose the Day or Sleep transition to adjust in this schedule.');
  }
  var changed = false;
  for (final tr in timed) {
    final t = tr['trigger'];
    if (t is! Map || tr['trigger_enabled'] == false) continue;
    if (t['kind'] == 'solar') {
      final minutes = (t['offset_minutes'] as int? ?? 0) + delta;
      if (minutes.abs() > 720) {
        throw const FormatException('Solar offset outside supported range');
      }
      t['offset_minutes'] = minutes;
      changed = true;
    } else if (t['kind'] == 'scheduled') {
      final parts = (t['time'] as String).split(':').map(int.parse).toList();
      final m = (parts[0] * 60 + parts[1] + delta) % 1440;
      t['time'] =
          '${(m ~/ 60).toString().padLeft(2, '0')}:${(m % 60).toString().padLeft(2, '0')}';
      changed = true;
    }
  }
  if (!changed) throw const FormatException('No enabled timed transitions');
  return next;
}
