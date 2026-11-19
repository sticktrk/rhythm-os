import 'dart:async';

import 'package:dio/dio.dart';
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart' show DirectHubAccess;
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:uuid/uuid.dart';

import '../../providers/home_provider.dart';
import '../../providers/server_sync_provider.dart';
import 'device_pairing_flow.dart';
import 'assistant_tools_screen.dart';
import 'customer_assistant_screen.dart';
import '../../services/customer_assistant_service.dart';

typedef _Args = Map<String, dynamic>;
typedef AssistantMessage = ({String text, bool resident});

/// A local, guided conversation. Replies, room names, and walk evidence stay
/// in this screen. Every operation uses the selected hub's authenticated SDK.
class AssistantChatScreen extends StatefulWidget {
  const AssistantChatScreen({
    super.key,
    required this.api,
    required this.serverId,
    required this.isCurrentTarget,
    required this.targetChanges,
    required this.openSetup,
    this.history,
    this.openAccountTools,
  });

  final List<AssistantMessage>? history;
  final Future<void> Function(BuildContext, List<AssistantMessage>)?
      openAccountTools;
  final RhythmServerApi Function() api;
  final String serverId;
  final bool Function() isCurrentTarget;
  final Listenable targetChanges;
  final Future<void> Function(BuildContext) openSetup;

  static Future<void> show(BuildContext context,
      {List<AssistantMessage>? history,
      bool Function()? accountIsCurrent}) async {
    final sync = context.read<ServerSyncProvider>();
    final homes = context.read<HomeProvider>();
    final identity = sync.connectedServerInstanceId;
    final selected = homes.activeServerHub?.id;
    final home = homes.currentHome?.id;
    final direct = DirectHubAccess.capture();
    if (identity == null || !sync.connection.connected) return;
    await Navigator.of(context).push<void>(MaterialPageRoute(
      builder: (_) => AssistantChatScreen(
        history: history,
        openAccountTools:
            history == null && CustomerAssistantService.signedIn() != null
                ? (ctx, messages) =>
                    CustomerAssistantScreen.show(ctx, history: messages)
                : null,
        api: () => sync.connection.api,
        serverId: identity,
        isCurrentTarget: () =>
            (accountIsCurrent?.call() ?? true) &&
            direct.isSameSelection &&
            sync.connection.connected &&
            homes.currentHome?.id == home &&
            homes.activeServerHub?.id == selected &&
            sync.connectedServerInstanceId == identity,
        targetChanges: Listenable.merge([sync, homes, DirectHubAccess.changes]),
        openSetup: (context) => startDevicePairingFlow(context,
            analyticsSource: 'lighting_assistant'),
      ),
    ));
  }

  @override
  State<AssistantChatScreen> createState() => _AssistantChatScreenState();
}

enum _Step {
  request,
  room,
  sensor,
  walk,
  handoff,
  review,
  lightingReview,
  lightingAction,
  retry
}

class _AssistantChatScreenState extends State<AssistantChatScreen> {
  late final _messages = widget.history ?? <AssistantMessage>[];
  final _text = TextEditingController();
  final _scroll = ScrollController();
  late RhythmAssistantContract _contract;
  var _nodes = <RhythmTopologyNode>[];
  _Step _step = _Step.retry;
  bool _busy = true;
  String _intent = '';
  RhythmTopologyNode? _room;
  String _sensor = '', _walk = '';
  String? _trial, _trialRoom;
  RhythmMotionTargetReview? _review;
  RhythmMotionDiagnosis? _diagnosis;
  String? _lightPlan, _lightApply, _lightActionLabel;
  _Args _lightArguments = {};
  String _lightOperation = '';
  int _trialMinutes = -60;

  bool get _active => mounted && widget.isCurrentTarget();
  Iterable<RhythmTopologyNode> get _rooms => _nodes.where((n) => n.isRoom);
  Iterable<RhythmTopologyNode> get _sensors =>
      _nodes.where((n) => n.kind == RhythmNodeKind.motionSensor);

  @override
  void initState() {
    super.initState();
    widget.targetChanges.addListener(_targetChanged);
    unawaited(_run(_discover));
  }

  void _targetChanged() {
    if (mounted) setState(() {});
  }

  @override
  void dispose() {
    widget.targetChanges.removeListener(_targetChanged);
    _text.dispose();
    _scroll.dispose();
    super.dispose();
  }

  void _say(String text, {bool resident = false}) {
    if (!_active) return;
    setState(() => _messages.add((text: text, resident: resident)));
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && _scroll.hasClients) {
        _scroll.jumpTo(_scroll.position.maxScrollExtent);
      }
    });
  }

  Future<void> _run(Future<void> Function() action) async {
    if (!_active) return;
    setState(() => _busy = true);
    try {
      await action();
    } on DioException catch (error) {
      if (_active) {
        final body = error.response?.data;
        final detail = body is Map ? body['error'] : null;
        final message = detail is Map ? detail['message'] : null;
        _say(message is String
            ? message
            : 'The hub could not verify this request. Refresh before trying again.');
        _step = _Step.retry;
        _review = null;
        _lightPlan = null;
      }
    } catch (_) {
      if (_active) {
        _say(
            'I could not verify the result on this hub. Refresh its state before trying again.');
        _step = _Step.retry;
        _review = null;
        _lightPlan = null;
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _discover() async {
    final contract = await widget.api().getAssistantContract();
    if (!_active) return;
    if (contract == null ||
        !contract.operations.any((op) => op.tier == RhythmAssistantTier.read)) {
      _say(
          'This hub does not advertise guided lighting or motion tests. You can still use its usual controls.');
      _step = _Step.retry;
      return;
    }
    _contract = contract;
    await _readTopology();
    if (!_active) return;
    _step = _Step.request;
    _say(
        'What would you like to try? You can check why a room stayed dark, connect a motion sensor, or adjust the light in a room.');
  }

  Future<void> _readTopology() async {
    if (!_active) throw StateError('Hub selection changed');
    final snapshot = await widget.api().getAssistantTopologySnapshot();
    if (!_active || snapshot.serverInstanceId != widget.serverId) {
      throw StateError('Hub identity changed');
    }
    _nodes = snapshot.nodes;
  }

  Future<_Args> _call(String id, _Args args) async {
    if (!_active) throw StateError('Hub selection changed');
    final result =
        await widget.api().callAssistantOperation(_contract, id, args);
    if (!_active ||
        (result.containsKey('server_instance_id') &&
            result['server_instance_id'] != widget.serverId)) {
      throw StateError('Hub identity changed');
    }
    return result;
  }

  String _name(String? id) {
    if (id == null) return 'unassigned';
    for (final node in _nodes) {
      if (node.id == id) return node.name.isEmpty ? id : node.name;
    }
    return id;
  }

  RhythmTopologyNode? _match(
      String message, Iterable<RhythmTopologyNode> nodes) {
    final matches = nodes.where((n) {
      final name = n.name.toLowerCase().trim();
      return n.id == message ||
          (name.isNotEmpty &&
              RegExp(
                r'(?:^|\W)' + RegExp.escape(name) + r'(?:$|\W)',
              ).hasMatch(message.toLowerCase()));
    }).toList();
    return matches.length == 1 ? matches.single : null;
  }

  Future<void> _reply(String reply, {RhythmTopologyNode? selected}) async {
    if (_busy || !_active || reply.trim().isEmpty) return;
    _say(reply.trim(), resident: true);
    _text.clear();
    final lower = reply.toLowerCase().trim();
    await _run(() async {
      switch (_step) {
        case _Step.lightingAction:
          if (lower == 'cancel') {
            _step = _Step.request;
            _say('Cancelled.');
          } else if (reply == _lightActionLabel) {
            final result = await _call(_lightOperation, _lightArguments);
            if (result['trial_id'] is String) {
              _trial = result['trial_id'] as String;
              _trialRoom = _room!.id;
            } else if (_lightOperation == 'undo_room_trial') {
              _trial = null;
            }
            _say('The hub acknowledged the request.');
            _reportLights(result);
            _step = _Step.request;
          } else {
            _say('Choose the proposed action or cancel it.');
          }
        case _Step.lightingReview:
          if (lower == 'cancel') {
            _lightPlan = null;
            _say('Cancelled the proposal.');
            _step = _Step.request;
          } else if (lower == 'save reviewed change' && _lightPlan != null) {
            final plan = _lightPlan!;
            _lightPlan = null;
            final result =
                await _call(_lightApply!, {'plan_id': plan, 'confirmed': true});
            _say(result['canonical_readback_verified'] == true
                ? 'The hub verified the saved change.'
                : 'Read the profile again to verify the saved change.');
            _step = _Step.request;
          }
        case _Step.retry:
          await _discover();
        case _Step.request:
          _intent = lower;
          if (!(RegExp(
                  'motion|sensor|walk|stayed (off|dark)|didn.t turn on|brighter|dimmer|warmer|earlier|normal|undo|explain|why|doing|save|absorb|dusk|sunset|evening')
              .hasMatch(lower))) {
            _say(
                'Try “a little brighter”, “why is it dim?”, “one hour earlier”, or “hook up this sensor”.');
            return;
          }
          _room = _match(lower, _rooms);
          if (_room == null &&
              (_intent == 'undo trial' ||
                  _intent == 'undo' ||
                  _intent == 'save this look')) {
            _room = _nodes.where((n) => n.id == _trialRoom).firstOrNull;
          }
          if (_room != null) {
            await _roomSelected();
          } else {
            _step = _Step.room;
            _say('Which room do you mean?');
          }
        case _Step.room:
          _room = selected ?? _match(lower, _rooms);
          if (_room == null) {
            _say('Choose a room from this hub.');
          } else {
            await _roomSelected();
          }
        case _Step.sensor:
          final node = selected ?? _match(lower, _sensors);
          if (node == null && lower != 'my sensor is missing') {
            _say('Choose a sensor, or select “My sensor is missing”.');
            return;
          }
          _sensor = node?.id ?? '';
          await _startWalk();
        case _Step.handoff:
          if (lower == 'open device setup') await widget.openSetup(context);
          if (!_active) return;
          await _readTopology();
          _say('Which sensor is visible now?');
          _step = _Step.sensor;
        case _Step.walk:
          switch (lower) {
            case 'review room link':
              await _plan();
            case 'walk again':
              await _startWalk();
            case 'check walk':
              await _inspect();
            default:
              _say('Choose a walk action below.');
          }
        case _Step.review:
          final reviewed = _review;
          if (lower == 'cancel') {
            _review = null;
            _step = _Step.walk;
            _say('Cancelled the proposed room link.');
          } else if (lower == 'confirm room link' && reviewed != null) {
            _review = null;
            final result = await _call(
                RhythmAssistantOperationId.applyMotionTarget,
                reviewed.confirmedArguments());
            if (result['canonical_readback_verified'] != true) {
              throw StateError('Room link requires readback');
            }
            _say(
                'The hub verified the room link. Walk past the sensor again to test it.');
            await _startWalk();
          } else {
            _say('Confirm or cancel the reviewed room link.');
          }
      }
    });
  }

  Future<void> _roomSelected() async {
    if (RegExp('motion|sensor|walk|stayed (off|dark)|didn.t turn on')
        .hasMatch(_intent)) {
      if (!_contract
          .supportsOperation(RhythmAssistantOperationId.startMotionWalk)) {
        _say('This hub does not advertise the motion walk test.');
        _step = _Step.request;
        return;
      }
      _step = _Step.sensor;
      _say('Which motion sensor should control ${_room!.name}?');
      return;
    }
    final room = {'room_id': _room!.id};
    final current = await _call('explain_room_output', room);
    final mode = current['mode'],
        state = current['state'],
        profile = current['profile_id'];
    _say('${_room!.name} is in $mode mode ($state) using $profile. '
        'The current target is ${current['canonical_brightness']}% at ${current['canonical_kelvin']} K.');
    if (_intent.contains('save this look') || _intent.contains('absorb')) {
      if (_trialRoom != _room!.id || _trial == null) {
        _say('Try a time-shifted look in this room before saving it.');
        _step = _Step.request;
        return;
      }
      await _planLighting(
          'plan_absorb_offset', {...room, 'offset_minutes': _trialMinutes});
      return;
    }
    if (RegExp('dusk|sunset|evening').hasMatch(_intent)) {
      if (current['state'] != 'active' ||
          current['adaptation_enabled'] != true) {
        _say(
            'This room is not actively following its brightness curve. Check its current mode and scheduled behavior first.');
        _step = _Step.request;
        return;
      }
      // Suggest a slower brightness-only ramp. Inspect both samples before
      // offering to save it: the room may already have a slower ramp.
      await _planLighting('plan_room_tuning',
          {...room, 'setting': 'evening_brightness_width', 'value': 0.2});
      return;
    }
    if (_intent.contains('why') ||
        _intent.contains('explain') ||
        _intent.contains('doing')) {
      _say(
          'Time offset: ${current['time_offset_minutes']} minutes. Brightness offset: ${current['brightness_offset']}.');
      _reportLights(current);
      _step = _Step.request;
      return;
    }
    String op, label;
    _Args args;
    if (_intent.contains('earlier')) {
      final minutes = RegExp(r'(\d+)\s*(minute|hour)').firstMatch(_intent);
      _trialMinutes = minutes == null
          ? -60
          : -int.parse(minutes[1]!) * (minutes[2] == 'hour' ? 60 : 1);
      op = 'try_room_offset';
      args = {...room, 'offset_minutes': _trialMinutes};
      label = 'Try ${_trialMinutes.abs()} minutes earlier';
    } else if (_intent.contains('normal') || _intent.contains('undo')) {
      if (_trial != null && _trialRoom == _room!.id) {
        op = 'undo_room_trial';
        args = {...room, 'trial_id': _trial};
        label = 'End this trial';
      } else {
        op = 'return_room_to_schedule';
        args = {...room, 'behavior': 'resume'};
        label = 'Resume adaptation';
      }
    } else if (_intent.contains('warmer') &&
        current['canonical_kelvin'] is num) {
      final kelvin = ((current['canonical_kelvin'] as num).round() - 200)
          .clamp(500, 25000);
      op = 'adjust_room_warmth';
      args = {...room, 'kelvin': kelvin};
      label = 'Try $kelvin K at this brightness';
    } else if (_intent.contains('brighter') || _intent.contains('dimmer')) {
      final delta = _intent.contains('brighter') ? 5 : -5;
      op = 'adjust_room_brightness';
      args = {...room, 'delta_percent': delta};
      label = 'Adjust brightness by ${delta > 0 ? '+' : ''}$delta points';
    } else {
      _say('Choose a brightness adjustment or ask what is driving this room.');
      _step = _Step.request;
      return;
    }
    final descriptor = _contract.operation(op);
    if (descriptor?.tier != RhythmAssistantTier.temporaryLightOutput) {
      _say('The hub does not advertise this adjustment.');
      _step = _Step.request;
      return;
    }
    _lightOperation = op;
    _lightActionLabel = label;
    _lightArguments = args;
    _say('${_room!.name} · ${descriptor!.description}');
    _step = _Step.lightingAction;
  }

  void _reportLights(_Args result) {
    final entries = (result['light_readback'] as List? ?? []).whereType<Map>();
    if (entries.isEmpty) _say('No fresh device readback is available.');
    for (final entry in entries) {
      final observed = entry['observed'];
      final name = _name(entry['light_id'] as String?);
      if (entry['control_scope'] == 'outside_ha_selection') {
        _say('$name · controlled by Home Assistant');
      } else if (entry['verification'] == 'fresh_device_readback' &&
          observed is Map) {
        final details = [
          if (observed['on'] is bool) observed['on'] ? 'on' : 'off',
          if (observed['brightness'] is num)
            "${observed['brightness']}% brightness",
          if (observed['kelvin'] is num) "${observed['kelvin']} K",
          if (observed['rgb'] is List) "RGB ${observed['rgb'].join(', ')}",
          if (observed['xy'] is List && observed['rgb'] == null)
            "xy ${observed['xy'].join(', ')}",
        ];
        _say(
            '$name · ${details.isEmpty ? 'no fresh device readback' : details.join(', ')}');
      } else {
        _say('$name · no fresh device readback');
      }
    }
  }

  Future<void> _planLighting(String id, _Args args) async {
    final preview = await _call(id, args);
    final rooms = (preview['affected_rooms'] as List)
        .whereType<Map>()
        .map((r) => _name(r['room_id'] as String?))
        .join(', ');
    final offsets = (preview['cleared_offsets'] as List)
        .whereType<Map>()
        .map((o) =>
            '${_name(o['node_id'] as String?)} (${o['before_minutes']} min)')
        .join(', ');
    final samples = (preview['samples'] as List).whereType<Map>().map((s) {
      final before = s['before'] as Map, after = s['after'] as Map;
      final minutes = ((before['hour'] as num) * 60).round() % 1440;
      final clock =
          '${minutes ~/ 60}:${(minutes % 60).toString().padLeft(2, '0')}';
      return '$clock · ${before['brightness']}% / ${before['kelvin']} K → ${after['brightness']}% / ${after['kelvin']} K';
    });
    _say(
        'Profile: ${preview['profile_id']}\nAffected rooms: $rooms\nOffsets cleared: ${offsets.isEmpty ? 'none' : offsets}\n${samples.join('\n')}');
    if (id == 'plan_room_tuning' &&
        !(preview['samples'] as List).whereType<Map>().any((s) =>
            (s['after']['brightness'] as num) >
            (s['before']['brightness'] as num))) {
      _say(
          'This preview does not make the sampled evening times brighter. The room setting is unchanged.');
      _step = _Step.request;
      return;
    }
    _lightApply =
        id == 'plan_room_tuning' ? 'apply_room_tuning' : 'apply_absorb_offset';
    if (preview['operation'] != _lightApply ||
        preview['room_id'] != _room!.id ||
        preview['server_instance_id'] != widget.serverId) {
      throw StateError('Review target changed');
    }
    _lightPlan = preview['plan_id'] as String;
    _step = _Step.lightingReview;
  }

  Future<void> _startWalk() async {
    final result = await _call(RhythmAssistantOperationId.startMotionWalk,
        {'room_id': _room!.id, 'sensor_id': _sensor});
    if (_showHandoff(result)) return;
    final integrations = (result['integrations'] as List? ?? []).join(', ');
    final targets = (result['control_targets'] as List? ?? [])
        .cast<String>()
        .map(_name)
        .join(', ');
    _say('Physical room: ${_name(result['physical_room_id'] as String?)}\n'
        'Controls: ${targets.isEmpty ? 'no rooms reported' : targets}'
        '${integrations.isEmpty ? '' : '\nIntegration: $integrations'}');
    _diagnosis = null;
    _walk = result['walk_id'] as String;
    _say(
        'Walk past ${_name(_sensor)} within ${result['window_seconds']} seconds, then check the walk.');
    _step = _Step.walk;
  }

  bool _showHandoff(_Args result) {
    if (result['status'] == 'handoff') {
      final handoff = result['handoff'] as Map;
      _say(
          '${handoff['actor']} · ${handoff['surface']}\n${handoff['instruction']}');
      _step = _Step.handoff;
      return true;
    }
    return false;
  }

  Future<void> _inspect({String? confirmedLight}) async {
    final result = RhythmMotionDiagnosis.fromJson(
        await _call(RhythmAssistantOperationId.inspectMotionWalk, {
      'walk_id': _walk,
      if (confirmedLight != null)
        'resident_observations': [
          {'device_id': confirmedLight, 'power_on': true}
        ],
    }));
    _diagnosis = result;
    _say([
      result.stage.replaceAll('_', ' '),
      if (result.detectedAtEpochMs != null)
        'Detected ${DateTime.fromMillisecondsSinceEpoch(result.detectedAtEpochMs!).toLocal()}',
      if (result.routedTargets.isNotEmpty)
        'Routed to ${result.routedTargets.map(_name).join(', ')}',
      if (result.reason != null) result.reason!.replaceAll('_', ' '),
      result.nextCheck
    ].join('\n'));
    for (final light in result.lights) {
      final observation = light.freshPowerObserved
          ? 'reported ${light.powerOn! ? 'on' : 'off'}'
          : 'no fresh device readback';
      _say(
          '${_name(light.deviceId)} · ${light.availability} · $observation${light.residentPowerOn == true ? ' · resident saw it turn on' : ''}');
    }
  }

  Future<void> _plan() async {
    final result = await _call(RhythmAssistantOperationId.planMotionTarget, {
      'room_id': _room!.id,
      'sensor_id': _sensor,
      'correlation_id': const Uuid().v4()
    });
    if (_showHandoff(result)) return;
    if (result['status'] == 'already_linked') {
      _say(
          'The hub already routes this sensor to ${_room!.name}. Test it with another walk.');
      return;
    }
    final review = RhythmMotionTargetReview.fromJson(result);
    if (review.room != _room!.id ||
        review.sensor != _sensor ||
        review.hub != widget.serverId) {
      throw StateError('Review target changed');
    }
    _review = review;
    String rooms(List<String> ids) =>
        ids.isEmpty ? 'none' : ids.map(_name).join(', ');
    _say(
        'Sensor: ${_name(review.sensor)}\nPhysical room: ${_name(review.physicalRoom)}\n'
        'Current targets: ${rooms(review.before)}\nProposed targets: ${rooms(review.after)}');
    _step = _Step.review;
  }

  @override
  Widget build(BuildContext context) {
    final enabled = !_busy && _active;
    final choices = switch (_step) {
      _Step.request => [
          'Check a motion sensor',
          'A little brighter',
          if (_trial != null) ...['Undo trial', 'Save this look']
        ],
      _Step.walk => ['Check walk', 'Review room link', 'Walk again'],
      _Step.handoff => ['Open device setup', 'Check discovery'],
      _Step.lightingAction => [_lightActionLabel!, 'Cancel'],
      _Step.lightingReview => ['Save reviewed change', 'Cancel'],
      _Step.review => ['Confirm room link', 'Cancel'],
      _Step.retry => ['Refresh hub'],
      _Step.sensor => ['My sensor is missing'],
      _Step.room => <String>[],
    };
    final candidates = switch (_step) {
      _Step.sensor => _sensors,
      _Step.room => _rooms,
      _ => <RhythmTopologyNode>[],
    };
    return Scaffold(
      appBar: AppBar(title: const Text('Lighting assistant'), actions: [
        if (widget.openAccountTools != null)
          IconButton(
              tooltip: 'Account and Home tools',
              icon: const Icon(Icons.home_outlined),
              onPressed: _busy
                  ? null
                  : () => widget.openAccountTools!(context, _messages)),
        if (enabled && _step != _Step.retry)
          IconButton(
              icon: const Icon(Icons.build_outlined),
              tooltip: 'Browse hub tools',
              onPressed: () => Navigator.push<void>(
                  context,
                  MaterialPageRoute(
                      builder: (_) => AssistantToolsScreen(
                          contract: _contract,
                          call: _call,
                          serverId: widget.serverId,
                          isCurrentTarget: widget.isCurrentTarget,
                          targetChanges: widget.targetChanges)))),
      ]),
      body: SafeArea(
          child: Column(children: [
        Expanded(
            child: ListView(
                controller: _scroll,
                padding: const EdgeInsets.all(16),
                children: [
              for (final message in _messages)
                Align(
                    alignment: message.resident
                        ? Alignment.centerRight
                        : Alignment.centerLeft,
                    child: Card(
                        child: Padding(
                            padding: const EdgeInsets.all(12),
                            child: SelectableText(message.text))))
            ])),
        if (!_active)
          const Padding(
              padding: EdgeInsets.all(12),
              child: Text('Reconnect to the same hub to continue.')),
        if (_busy) const LinearProgressIndicator(),
        if (enabled)
          ConstrainedBox(
              constraints: const BoxConstraints(maxHeight: 160),
              child: SingleChildScrollView(
                  child: Wrap(spacing: 8, children: [
                for (final node in candidates)
                  ActionChip(
                      label: Text(node.name.isEmpty ? node.id : node.name),
                      tooltip: node.id,
                      onPressed: () => _reply(_name(node.id), selected: node)),
                if (_step == _Step.walk &&
                    _diagnosis != null &&
                    {
                      'command_attempted',
                      'command_acknowledged',
                      'command_not_needed',
                      'command_uncertain'
                    }.contains(_diagnosis!.stage))
                  for (final light in _diagnosis!.lights
                      .where((l) => l.residentPowerOn != true))
                    ActionChip(
                        label: Text('I saw ${_name(light.deviceId)} turn on'),
                        onPressed: () => _run(
                            () => _inspect(confirmedLight: light.deviceId))),
                for (final choice in choices)
                  ActionChip(
                      label: Text(choice), onPressed: () => _reply(choice)),
              ]))),
        Padding(
            padding: const EdgeInsets.all(12),
            child: TextField(
                controller: _text,
                enabled: enabled,
                textInputAction: TextInputAction.send,
                decoration: InputDecoration(
                    labelText: 'Message',
                    suffixIcon: IconButton(
                        icon: const Icon(Icons.send),
                        tooltip: 'Send',
                        onPressed: enabled ? () => _reply(_text.text) : null)),
                onSubmitted: _reply)),
      ])),
    );
  }
}
