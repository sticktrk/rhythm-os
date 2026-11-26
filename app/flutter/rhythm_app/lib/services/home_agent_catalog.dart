import 'dart:convert';
import 'package:crypto/crypto.dart';
import 'package:flutter/painting.dart' show Color, HSVColor;
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:uuid/validation.dart';
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
      this.proof, this.digest,
      {this.title, String? detail})
      : detail = detail ?? description;
  final AgentTarget target;
  final String operation, description, digest;
  final CustomerAction arguments, proof;

  /// The proposed light value, for changes that have one to show.
  final String? title;

  /// What the change does, without repeating the target or [title].
  final String detail;
}

class HomeAgentCatalog {
  HomeAgentCatalog._(
      this.hub, this.targets, this.generatedSceneTarget, this._savedScenes);
  final LightingChatHub hub;
  final Map<String, AgentTarget> targets;
  final AgentTarget? generatedSceneTarget;
  final Map<String, CustomerAction> _savedScenes;
  bool get canCreateHomeScene => generatedSceneTarget != null;

  late final Map<String, RhythmTopologyNode> _roomNodes = {
    for (final room in hub.rooms) room.id: room
  };
  late final Map<String, AgentTarget> _roomsById = {
    for (final t in targets.values.where((t) => t.kind == 'r')) t.id: t
  };
  late final List<AgentTarget> rooms =
      List.unmodifiable(_roomsById.values.toList()..sort(_roomOrder));

  List<AgentTarget> roomsMatching(String query) {
    final words = LightingChatHub.words(query);
    if (words.isEmpty) return [];
    final location = words
        .replaceFirst(RegExp(r' please$'), '')
        .replaceFirst(RegExp(r'^in '), '');
    final phrase = location.replaceFirst(RegExp(r'^(?:the|my|our) '), '');
    for (final candidate in {words, location, phrase}) {
      final exact = rooms
          .where((room) =>
              LightingChatHub.words(room.name) == candidate ||
              LightingChatHub.words(targetLabel(room)) == candidate)
          .toList();
      if (exact.isNotEmpty) return exact;
    }
    if (const {'room', 'rooms', 'home', 'house', 'lights'}.contains(phrase)) {
      return [];
    }
    return rooms
        .where((room) =>
            ' ${LightingChatHub.words(room.name)} '.contains(' $phrase '))
        .toList();
  }

  int _roomOrder(AgentTarget a, AgentTarget b) =>
      LightingChatHub.roomOrder(_roomNodes[a.id]!, _roomNodes[b.id]!);

  String targetLabel(AgentTarget target) {
    final node = target.kind == 'r' ? _roomNodes[target.id] : null;
    return node == null ? target.name : hub.roomLabel(node);
  }

  AgentTarget? roomAnswer(String text, List<AgentTarget> choices) {
    final match = hub.roomAnswer(text, [
      for (final choice in choices)
        if (_roomNodes[choice.id] case final node?) node
    ]);
    return _roomsById[match?.id];
  }

  static Future<HomeAgentCatalog> load(
      LightingChatHub hub, HomeAgentClient client) async {
    final targets = <String, AgentTarget>{};
    final savedScenes = <String, CustomerAction>{};
    void add(String kind, String id, String name, Set<String> tools) {
      if (id.isEmpty) return;
      final ref = client.reference(kind, id);
      targets[ref] =
          AgentTarget(ref, id, name.isEmpty ? id : name, kind, tools);
    }

    for (final r in hub.rooms) {
      add('r', r.id, r.name.trim().isEmpty ? 'Unnamed room' : r.name.trim(),
          hub.tools);
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
            if (kind == 'c') savedScenes[row['id']] = CustomerAction.from(row);
          }
        }
      }
    }
    const sceneTools = {
      'read_automation_settings',
      'plan_scene_edit',
      'apply_scene_edit',
      'plan_home_scene_edit',
      'apply_home_scene_edit'
    };
    final draft = hub.catalog.containsAll(sceneTools)
        ? AgentTarget(client.reference('d', 'new-scene'), 'new-scene',
            'New whole-home scene', 'd', {'plan_generated_home_scene'})
        : null;
    return HomeAgentCatalog._(hub, targets, draft, savedScenes);
  }

  static String generatedSceneId(String planId) {
    if (!UuidValidation.isValidUUID(fromString: planId)) {
      throw const FormatException('A fresh scene review is required.');
    }
    return 'chat-${planId.toLowerCase()}';
  }

  /// Resolve the reviewed creation after refreshing the hub's saved settings.
  /// An edited scene with the same ID requires a new resident decision.
  AgentTarget? savedGeneratedScene(String planId, CustomerAction arguments) {
    if (!canCreateHomeScene) return null;
    final scene = _generatedScene(planId, arguments);
    final stored = _savedScenes[scene['id']];
    if (!_sameJson(scene, stored)) return null;
    return targets.values
        .where((t) => t.kind == 'c' && t.id == scene['id'])
        .firstOrNull;
  }

  static bool _sameJson(Object? expected, Object? actual) {
    if (expected is Map && actual is Map) {
      return expected.length == actual.length &&
          expected.keys.every((k) =>
              actual.containsKey(k) && _sameJson(expected[k], actual[k]));
    }
    if (expected is List && actual is List) {
      return expected.length == actual.length &&
          expected.indexed
              .every((entry) => _sameJson(entry.$2, actual[entry.$1]));
    }
    return expected == actual;
  }

  static CustomerAction _generatedScene(String planId, CustomerAction args) {
    final id = generatedSceneId(planId);
    final brightness = args['brightness_percent'];
    final colorCount = args.length - 1;
    if (brightness is! int ||
        brightness < 1 ||
        brightness > 100 ||
        colorCount < 2 ||
        colorCount > 4) {
      throw const FormatException(
          'Choose two to four colors and a brightness from 1 to 100%.');
    }
    final palette = <RhythmLightSceneOutput>[];
    final names = <String>{};
    for (var i = 1; i <= colorCount; i++) {
      final hex = args['color_$i'];
      if (hex is! String || !RegExp(r'^#[a-fA-F0-9]{6}$').hasMatch(hex)) {
        throw const FormatException(
            'Each scene color must be a six-digit hex color.');
      }
      final rgb = int.parse(hex.substring(1), radix: 16);
      names.add(_colorName(HSVColor.fromColor(Color(0xff000000 | rgb))));
      palette.add(RhythmLightSceneOutput.on(
          brightness: brightness,
          color: RhythmLightColor.rgb(RhythmSceneRgbColor(
              r: rgb >> 16, g: rgb >> 8 & 255, b: rgb & 255))));
    }
    final scene = RhythmSceneDefinition(
        id: id,
        name: names.join(' & '),
        light: RhythmLightScene(paletteMode: 'cycle', palette: palette),
        extensions: const {'whole_home': true}).toJson();
    // Match the hub's SceneDefinition and LightSceneLayer serialization for
    // optional fields (the SDK emits null for description and default_output).
    scene.removeWhere((_, value) => value == null);
    (scene['light'] as Map).removeWhere((_, value) => value == null);
    return scene;
  }

  static String _colorName(HSVColor color) {
    if (color.value < 0.1) return 'Black';
    if (color.saturation < 0.12) return color.value < 0.7 ? 'Gray' : 'White';
    return switch (color.hue) {
      < 16 || >= 345 => 'Red',
      < 42 => 'Orange',
      < 68 => 'Gold',
      < 155 => 'Green',
      < 195 => 'Teal',
      < 255 => 'Blue',
      < 300 => 'Purple',
      _ => 'Pink'
    };
  }

  late final Map<String, List<AgentTarget>> _names = _targetNames();

  Map<String, List<AgentTarget>> _targetNames() {
    final groups = <String, List<AgentTarget>>{};
    for (final target in targets.values) {
      final aliases = {target.name.toLowerCase()};
      if (target.kind == 'r') {
        aliases.add(targetLabel(target).toLowerCase());
      }
      for (final alias in aliases) {
        groups.putIfAbsent(alias, () => []).add(target);
      }
    }
    return groups;
  }

  // Match the complete name first: "Office (1)" precedes "Office", and
  // "Main Bedroom" precedes "Bedroom".
  late final RegExp _namePattern = RegExp(
      r'(?<![\p{L}\p{N}_])(' +
          (_names.keys.toList()..sort((a, b) => b.length.compareTo(a.length)))
              .map(RegExp.escape)
              .join('|') +
          r')(?![\p{L}\p{N}_])',
      caseSensitive: false,
      unicode: true);

  List<AgentTarget> ambiguousRooms(String prompt) {
    if (_names.isEmpty) return [];
    for (final match in _namePattern.allMatches(prompt)) {
      final group = _names[match[0]!.toLowerCase()]!;
      if (group.length > 1) {
        final rooms = group.where((t) => t.kind == 'r').toList();
        if (rooms.length > 1) return rooms..sort(_roomOrder);
      }
    }
    return [];
  }

  String resolveRoomMention(String prompt, AgentTarget chosen) {
    if (_names.isEmpty) return prompt;
    return prompt.replaceAllMapped(_namePattern, (match) {
      final original = match[0]!;
      final group = _names[original.toLowerCase()]!;
      return group.length > 1 && group.any((t) => t.ref == chosen.ref)
          ? chosen.ref
          : original;
    });
  }

  String redact(String text, [String? selected]) {
    if (_names.isEmpty) return text;
    return text.replaceAllMapped(_namePattern, (m) {
      final matches = _names[m[0]!.toLowerCase()]!;
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
    final whole = LightingChatHub.refersToWholeHome(prompt);
    const refPattern = r'[rscm]_[a-f0-9]{24}';
    final exclusions = RegExp(
        LightingChatHub.exclusionPrefix +
            r'((?:(?:the\s+)?' +
            refPattern +
            r')(?:(?:\s*,\s*(?:and\s+)?|\s+(?:and|or)\s+)(?:the\s+)?' +
            refPattern +
            r')*)',
        caseSensitive: false);
    final excluded = {
      for (final group in exclusions.allMatches(prompt))
        for (final match in RegExp(refPattern).allMatches(group[1]!)) match[0]
    };
    final hasExclusion =
        excluded.isNotEmpty || LightingChatHub.hasExclusion(prompt);
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
              (mentioned.isEmpty &&
                  !whole &&
                  !hasExclusion &&
                  t.ref == selected) ||
              (motion != null &&
                  (mentioned.isEmpty ||
                      mentioned.any((m) =>
                          m.ref == motion['room'] ||
                          m.ref == motion['sensor'])) &&
                  (t.ref == motion['room'] || t.ref == motion['sensor'])) ||
              (whole &&
                  (!hasExclusion || excluded.isNotEmpty) &&
                  t.kind == 'r'));
      var tools = inScope ? t.tools.toList() : <String>[];
      if (motion?['missing_target'] != true || motion?['sensor'] != t.ref) {
        tools.remove('plan_motion_target');
      }
      if (!whole || hasExclusion) tools.remove('plan_home_scene_edit');
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
    final target = step['target'] == generatedSceneTarget?.ref
        ? generatedSceneTarget
        : targets[step['target']];
    final op = step['operation'] as String;
    if (target == null || !target.tools.contains(op)) {
      throw StateError('Target or capability changed');
    }
    final args = CustomerAction.from(step['arguments'] as Map);
    CustomerAction proof = {};
    late CustomerAction dispatch;
    late String description, operation;
    String? title, detail;
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
      title = p.label;
      detail = p.detail;
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
      final generated = op == 'plan_generated_home_scene';
      final planOperation = generated ? 'plan_scene_edit' : op;
      if (generated) {
        final scene = _generatedScene(journey, args);
        params.addAll({'scene_id': scene['id'], 'definition': scene});
        title = scene['name'] as String;
      } else if (op == 'plan_home_scene_edit') {
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
      final preview = await hub.call(planOperation, params);
      final plan = preview['plan_id'];
      if (plan is! String || preview['server_instance_id'] != instance) {
        throw StateError('Fresh preview required');
      }
      operation = planOperation.replaceFirst('plan_', 'apply_');
      if (!hub.catalog.contains(operation)) throw StateError('Update the hub');
      final before = preview['before'];
      if (generated) {
        if (before is! Map ||
            !before.containsKey('current') ||
            before['current'] != null) {
          throw const FormatException(
              'This scene could already exist. Start a new scene review.');
        }
        if (!_sameJson(preview['proposal'], {
          'scene': {
            'id': params['scene_id'],
            'definition': params['definition']
          }
        })) {
          throw const FormatException(
              'The hub returned a different scene. Start a new review.');
        }
      } else if (op == 'plan_home_scene_edit' &&
          target.id.startsWith('chat-') &&
          UuidValidation.isValidUUID(fromString: target.id.substring(5)) &&
          (before is! Map ||
              !_sameJson(_savedScenes[target.id], before['current']))) {
        throw const FormatException(
            'The saved palette changed. Request a fresh review.');
      }
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
        'plan_generated_home_scene' => 'Save ${[
            for (var i = 1; i < args.length; i++) args['color_$i']
          ].join(', ')} at ${args['brightness_percent']}% brightness. You can review applying it to your home after it is saved.',
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
    return AgentReview(target, operation, dispatch, description, proof, digest,
        title: title, detail: detail);
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
