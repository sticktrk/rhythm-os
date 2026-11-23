import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';
import 'package:dio/dio.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart' show PlatformException;
import 'package:image_picker/image_picker.dart';
import 'package:uuid/uuid.dart';
import 'package:posthog_flutter/posthog_flutter.dart';
import '../../services/analytics_service.dart';
import '../../services/customer_assistant_service.dart';
import '../../services/home_agent_client.dart';
import '../../services/home_agent_catalog.dart';
import '../../services/lighting_chat_service.dart';
import '../../widgets/assistant_chat_widgets.dart';
import '../../widgets/solar_orbit.dart' show CelestialColors;

class HomeAgentChat extends StatefulWidget {
  const HomeAgentChat(
      {super.key,
      required this.client,
      required this.hub,
      required this.current,
      required this.guided,
      this.initialRoomId,
      this.pickPhoto = pickLightingPhoto});
  final HomeAgentClient client;
  final LightingChatHub hub;
  final bool Function() current;
  final VoidCallback guided;
  final String? initialRoomId;
  final Future<Uint8List?> Function(ImageSource) pickPhoto;
  @override
  State<HomeAgentChat> createState() => _HomeAgentChatState();
}

class _HomeAgentChatState extends State<HomeAgentChat> {
  final _input = TextEditingController(), _scroll = ScrollController();
  final _messages = <({bool resident, String text, bool share})>[];
  final _reviews = <int, AgentReview>{};
  HomeAgentCatalog? _catalog;
  CustomerAction? _plan, _motion;
  List<CustomerAction> _history = [];
  String? _lastResult;
  Uint8List? _photo;
  String? _focus;
  final _reviewErrors = <int, String>{};
  bool _busy = false;
  bool get _live => mounted && widget.current();
  List<CustomerAction> get _steps => (_plan?['steps'] as List? ?? [])
      .whereType<Map>()
      .map(CustomerAction.from)
      .toList();
  bool get _pending => _steps.any((s) => s['status'] == 'review');

  @override
  void initState() {
    super.initState();
    _run(_open);
  }

  @override
  void dispose() {
    _input.dispose();
    _scroll.dispose();
    super.dispose();
  }

  void _add(String text, {bool resident = false, bool share = false}) {
    if (!_live) return;
    setState(
        () => _messages.add((resident: resident, text: text, share: share)));
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && _scroll.hasClients) {
        _scroll.jumpTo(_scroll.position.maxScrollExtent);
      }
    });
  }

  Future<void> _run(Future<void> Function() fn) async {
    if (!_live || _busy) return;
    setState(() => _busy = true);
    try {
      await fn();
    } catch (e) {
      if (_live &&
          e is DioException &&
          [401, 403].contains(e.response?.statusCode)) {
        setState(() {
          _messages.clear();
          _history = [];
          _motion = null;
          _lastResult = null;
          _photo = null;
          _reviews.clear();
          _plan = null;
          _catalog = null;
          _input.clear();
        });
        _add(
            'Account or home access has changed. Reopen the assistant to check access.');
      } else if (_live) {
        _add(e is DioException && e.response?.statusCode == 429
            ? 'You’ve reached today’s chat limit. Guided tools are available.'
            : 'That step could not be verified. Refresh progress or continue in Guided tools.');
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _open() async {
    final catalog = await HomeAgentCatalog.load(widget.hub, widget.client);
    if (!_live) return;
    setState(() {
      _catalog = catalog;
      _focus = catalog.targets.values
          .where((t) => t.id == widget.initialRoomId)
          .firstOrNull
          ?.ref;
    });
    await _refresh();
  }

  Future<void> _refresh() async {
    final state = await widget.client.call('status');
    await _showState(state, prepare: true);
  }

  Future<void> _showState(CustomerAction state, {bool prepare = false}) async {
    if (!_live) return;
    final plan =
        state['plan'] is Map ? CustomerAction.from(state['plan']) : null;
    if (plan?['id'] != _plan?['id']) {
      _reviews.clear();
      _reviewErrors.clear();
    }
    setState(() {
      _plan = plan;
      _history = (state['history'] as List? ?? [])
          .whereType<Map>()
          .map(CustomerAction.from)
          .toList();
      _lastResult = state['lastResult'] as String?;
    });
    if (!prepare || plan == null || _catalog == null) return;
    for (final (index, step) in _steps.indexed) {
      if (step['status'] != 'review' || _reviews.containsKey(index)) continue;
      if ((plan['expires'] as num) < DateTime.now().millisecondsSinceEpoch) {
        break;
      }
      late AgentReview preview;
      try {
        preview =
            await _catalog!.review(step, widget.client.instance, plan['id']);
      } on FormatException catch (e) {
        if (_live) setState(() => _reviewErrors[index] = e.message);
        continue;
      }
      if (!_live) return;
      await widget.client.call('prepare',
          {'plan': plan['id'], 'index': index, 'digest': preview.digest});
      if (!_live) return;
      setState(() => _reviews[index] = preview);
    }
  }

  Future<void> _send(String text) => _run(() async {
        final catalog = _catalog;
        final prompt = text.trim(), photo = _photo;
        if (catalog == null || (prompt.isEmpty && photo == null)) return;
        final redacted = catalog.redact(prompt, _focus);
        if (redacted.contains('[ambiguous target]')) {
          _add(
              'That name matches more than one item. Pick the intended room before sending.');
          return;
        }
        final context = await catalog.context(redacted, _focus, _motion);
        if (!_live) return;
        final mentioned = catalog.targets.values
            .where((t) => t.kind == 'r' && redacted.contains(t.ref))
            .toList();
        if (mentioned.length == 1) _focus = mentioned.single.ref;
        final history = _messages.where((m) => m.share).toList();
        final clock = Stopwatch()..start();
        final turn = {
          'turn': const Uuid().v4(),
          'messages': [
            for (final m
                in history.skip(history.length > 7 ? history.length - 7 : 0))
              {
                'role': m.resident ? 'user' : 'assistant',
                'text': catalog.redact(m.text, _focus)
              },
            {'role': 'user', 'text': redacted}
          ],
          'targets': context,
          if (_motion != null) 'motion': _motion,
          'selected': _focus
        };
        _input.clear();
        setState(() => _photo = null);
        _add(prompt.isEmpty ? 'Shared a photo' : prompt,
            resident: true, share: prompt.isNotEmpty);
        try {
          final reply = await widget.client
              .call(photo == null ? 'turn' : 'photo', turn, photo);
          if (!_live) return;
          final answer = reply['text'];
          if (answer is String && answer.isNotEmpty) {
            var visible = answer;
            for (final t in catalog.targets.values) {
              visible = visible.replaceAll(t.ref, t.name);
            }
            _add(visible, share: true);
          }
          unawaited(AnalyticsService().logHomeAgentProgress(
              turn['turn'] as String, 'answered',
              latencyMs: clock.elapsedMilliseconds,
              steps: (reply['plan']?['steps'] as List?)?.length ?? 0));
          await _showState(reply, prepare: true);
          unawaited(AnalyticsService().logLightingChatTurn(
              reply['proposed'] == true
                  ? LightingChatOutcome.proposal
                  : LightingChatOutcome.reply));
        } catch (_) {
          if (_live) await _refresh();
          rethrow;
        }
      });
  Future<void> _apply() => _run(() async {
        final planId = _plan!['id'];
        unawaited(AnalyticsService()
            .logHomeAgentProgress(planId, 'approved', steps: _steps.length));
        for (final (i, step) in _steps.indexed) {
          if (!_live || step['status'] != 'review') continue;
          final preview = _reviews[i];
          if (preview == null) return;
          // Re-read current scope just before consuming permission to dispatch.
          // For saved settings the hub validates the original reviewed plan on apply.
          if (LightingProposal.supported.contains(preview.operation)) {
            final fresh =
                await _catalog!.review(step, widget.client.instance, planId);
            if (fresh.digest != preview.digest) {
              _add(
                  'The room’s managed lights changed. Skip this plan and request a new review.');
              return;
            }
          }
          final claim = await widget.client.call(
              'claim', {'plan': planId, 'index': i, 'digest': preview.digest});
          if (!_live) return;
          final nonce = claim['claim'];
          if (nonce is! String) {
            await _refresh();
            return;
          }
          await _showState(claim);
          String outcome = 'uncertain';
          String? receipt;
          try {
            final res =
                await widget.hub.call(preview.operation, preview.arguments);
            if (!_live) return;
            if (preview.operation == 'start_motion_walk' &&
                res['status'] == 'walk_started' &&
                res['walk_id'] is String) {
              outcome = 'walk_started';
              receipt = res['walk_id'];
            } else if (res['status'] == 'awaiting_owner_approval' &&
                res['plan_id'] is String) {
              outcome = 'awaiting_owner';
              receipt = res['plan_id'];
            } else if (res['server_instance_id'] == widget.client.instance &&
                (res['canonical_readback_verified'] == true ||
                    (res['status'] == 'applied' && res.containsKey('after')))) {
              outcome = 'verified';
            } else if (res['server_instance_id'] == widget.client.instance &&
                ['queued', 'partially_applied'].contains(res['status'])) {
              outcome = res['status'] == 'queued' ? 'queued' : 'partial';
            } else if (res['room_id'] == preview.target.id &&
                res['server_acknowledged'] == true) {
              final lights = (res['light_readback'] as List? ?? [])
                  .whereType<Map>()
                  .where((l) => l['control_scope'] != 'outside_ha_selection')
                  .toList();
              final fresh = lights
                  .where((l) =>
                      l['verification'] == 'fresh_device_readback' &&
                      l['observed'] is Map)
                  .length;
              outcome = lights.isNotEmpty && fresh == lights.length
                  ? 'observed'
                  : 'acknowledged';
              _add(
                  '${preview.target.name}: the hub accepted the adjustment. Fresh readings from $fresh of ${lights.length} managed lights.');
            }
          } catch (e) {
            if (e is DioException &&
                [400, 401, 403, 404, 409].contains(e.response?.statusCode)) {
              outcome = 'rejected';
            }
          }
          if (!_live) return;
          final reply = await widget.client.call('receipt', {
            'plan': planId,
            'index': i,
            'claim': nonce,
            'outcome': outcome,
            if (receipt != null) 'receipt': receipt
          });
          unawaited(AnalyticsService().logHomeAgentProgress(planId, outcome));
          await _showState(reply);
        }
      });
  Future<void> _inspect(CustomerAction step) => _run(() async {
        if (step['status'] == 'walk_started') {
          try {
            final data = await widget.hub
                .call('inspect_motion_walk', {'walk_id': step['receipt']});
            if (!_live ||
                data['server_instance_id'] != widget.client.instance) {
              return;
            }
            final rawStage = data['stage'];
            final stage = const {
              'detected',
              'routed',
              'suppressed',
              'command_not_needed',
              'command_queued',
              'command_attempted',
              'command_acknowledged',
              'no_command_issued',
              'command_uncertain'
            }.contains(rawStage)
                ? rawStage
                : 'waiting';
            final fresh = (data['lights'] as List? ?? [])
                .whereType<Map>()
                .where((l) => l['readback'] == 'fresh_device_readback')
                .length;
            _motion = {
              'sensor': step['target'],
              'room': step['arguments']['room'],
              'stage': stage,
              'detected': data['detected_at_epoch_ms'] != null,
              'missing_target': data['reason'] == 'missing_target',
              'fresh': fresh.clamp(0, 255)
            };
            final next = data['next_check'] ?? '';
            _add('Motion test: ${data['reason'] ?? data['status']}. $next',
                share: false);
            final lights =
                (data['lights'] as List? ?? []).whereType<Map>().toList();
            _add(
                'Detection: ${data['detected_at_epoch_ms'] == null ? 'none yet' : 'recorded'}. Routing: ${data['routed_targets'] is List ? (data['routed_targets'] as List).length : 0} targets. Fresh lamp readings: ${lights.where((l) => l['readback'] == 'fresh_device_readback').length}.');
          } catch (_) {
            _motion = null;
            _add(
                'This walk is unavailable or expired. Ask to start a fresh walk.');
          }
        } else if (step['status'] == 'awaiting_owner') {
          final res = await widget.hub
              .call('read_plan_outcome', {'plan_id': step['receipt']});
          _add('Owner review: ${res['status'] ?? 'unknown'}.');
        } else {
          final target = _catalog?.targets[step['target']];
          if (target?.kind == 'r') {
            final r = await widget.hub
                .call('explain_room_output', {'room_id': target!.id});
            _add(
                '${target.name} now targets ${r['canonical_brightness']}% at ${r['canonical_kelvin']} K. The earlier command outcome is uncertain.');
          } else {
            _add(
                'Read the current configuration in Guided tools before preparing another change.');
          }
        }
      });
  Future<void> _pick(ImageSource source) => _run(() async {
        try {
          final photo = await widget.pickPhoto(source);
          if (_live && photo != null) setState(() => _photo = photo);
        } on PlatformException {
          _add(source == ImageSource.camera
              ? 'The camera is unavailable. You can choose an existing photo or keep chatting.'
              : 'The photo library is unavailable. You can keep chatting.');
        } on FormatException {
          _add(
              'That image could not be attached. You can choose a smaller still or keep chatting.');
        }
      });
  Future<void> _clear() => _run(() async {
        await widget.client.call('clear');
        if (!_live) return;
        setState(() {
          _messages.clear();
          _history = [];
          _motion = null;
          _lastResult = null;
          _photo = null;
          _reviews.clear();
          _plan = null;
          _input.clear();
        });
        final reply = await widget.client.call('open');
        if (!_live) return;
        widget.client.session = reply['session'];
        await _open();
      });
  String _status(String status) => switch (status) {
        'review' => 'Awaiting review',
        'claimed' => 'Sent or interrupted · check outcome',
        'observed' => 'Hub accepted · fresh lamp readings',
        'acknowledged' => 'Hub accepted',
        'verified' => 'Configuration verified',
        'queued' => 'Hub queued the change',
        'partial' => 'Configuration changed · some lights unresolved',
        'uncertain' => 'Outcome uncertain',
        'walk_started' => 'Walk in progress',
        'awaiting_owner' => 'Awaiting hub owner',
        'rejected' => 'Hub rejected',
        'cancelled' => 'Skipped',
        _ => status
      };
  static const _disclosure =
      'Ask about your lights, or share a photo. Messages, readings and photos you send are processed by Cloudflare Workers AI.';
  static const _checkable = {
    'walk_started',
    'awaiting_owner',
    'uncertain',
    'claimed'
  };

  /// A readable name for a step the hub has not described yet.
  String _operation(String id) {
    final words =
        id.replaceFirst(RegExp(r'^(plan|apply)_'), '').replaceAll('_', ' ');
    return words.isEmpty ? words : words[0].toUpperCase() + words.substring(1);
  }

  Color _statusColor(String status) => switch (status) {
        'observed' || 'acknowledged' || 'verified' => const Color(0xFF22C55E),
        'claimed' ||
        'uncertain' ||
        'partial' ||
        'rejected' =>
          CelestialColors.warning,
        _ => CelestialColors.textSecondary
      };

  @override
  Widget build(BuildContext context) {
    final steps = _steps, plan = _plan, catalog = _catalog;
    final expired = plan != null &&
        (plan['expires'] as num) < DateTime.now().millisecondsSinceEpoch;
    final rooms =
        catalog?.targets.values.where((t) => t.kind == 'r').toList() ??
            const <AgentTarget>[];
    final earlier = [
      for (final old in _history.reversed)
        for (final row in (old['steps'] as List).whereType<Map>())
          if (_checkable.contains(row['status'])) CustomerAction.from(row)
    ];
    final canAsk = !_busy && catalog != null && !_pending;
    return PostHogMaskWidget(
        child: RhythmAssistantPage(
            title: 'Lighting assistant',
            actions: [
              PopupMenuButton<String>(
                  tooltip: 'More options',
                  icon: const Icon(Icons.more_horiz),
                  onSelected: (v) {
                    if (v == 'guided') {
                      widget.guided();
                    } else if (v == 'refresh') {
                      _run(_catalog == null ? _open : _refresh);
                    } else {
                      _clear();
                    }
                  },
                  itemBuilder: (_) => const [
                        PopupMenuItem(
                            value: 'guided', child: Text('Guided tools')),
                        PopupMenuItem(
                            value: 'refresh', child: Text('Refresh progress')),
                        PopupMenuItem(
                            value: 'clear', child: Text('Clear conversation'))
                      ])
            ],
            body: Column(children: [
              Expanded(
                  child: ListView(
                      controller: _scroll,
                      padding: const EdgeInsets.fromLTRB(20, 16, 20, 8),
                      children: [
                    if (_messages.isEmpty && plan == null && earlier.isEmpty)
                      LightingChatWelcome(
                          tools: widget.hub.tools,
                          onPrompt: canAsk ? _send : null,
                          disclosure: _disclosure)
                    else if (_messages.isEmpty)
                      const AssistantNote(_disclosure),
                    if (_messages.isEmpty && _lastResult == 'photo_advice')
                      const Padding(
                          padding: EdgeInsets.only(top: 8),
                          child: AssistantNote(
                              'A photo was discussed earlier. You can continue here or share it again if another look would help.')),
                    if (_messages.isEmpty) const SizedBox(height: 14),
                    for (final row in earlier)
                      Padding(
                          padding: const EdgeInsets.only(bottom: 10),
                          child: AssistantReviewCard(children: [
                            Row(children: [
                              Expanded(
                                  child: _StepStatus(
                                      title: catalog
                                              ?.targets[row['target']]?.name ??
                                          'Previous request',
                                      status: _status(row['status']),
                                      color: _statusColor(row['status']))),
                              TextButton(
                                  onPressed: _busy ? null : () => _inspect(row),
                                  child: const Text('Check progress')),
                            ]),
                          ])),
                    for (final m in _messages)
                      AssistantMessageBubble(
                          resident: m.resident, text: m.text),
                    if (plan != null && steps.isNotEmpty)
                      Padding(
                          padding: const EdgeInsets.only(bottom: 14),
                          child: AssistantReviewCard(children: [
                            for (final (i, s) in steps.indexed) ...[
                              if (i > 0)
                                const Divider(
                                    height: 29,
                                    thickness: 1,
                                    color: CelestialColors.orbitRing),
                              _AgentStep(
                                  target: _reviews[i]?.target.name ??
                                      catalog?.targets[s['target']]?.name ??
                                      'Target',
                                  title: _reviews[i]?.title ??
                                      _operation(s['operation']),
                                  detail: _reviews[i]?.detail,
                                  operation: s['operation'],
                                  arguments: s['arguments'] is Map
                                      ? CustomerAction.from(s['arguments'])
                                      : const {},
                                  status:
                                      _reviewErrors[i] ?? _status(s['status']),
                                  statusColor: _reviewErrors[i] != null
                                      ? CelestialColors.warning
                                      : _statusColor(s['status']),
                                  proof: switch (_reviews[i]) {
                                    final review?
                                        when !LightingProposal.supported
                                                .contains(review.operation) &&
                                            review.proof.isNotEmpty =>
                                      review.proof,
                                    _ => null
                                  },
                                  checkLabel: !_checkable.contains(s['status'])
                                      ? null
                                      : s['status'] == 'walk_started'
                                          ? 'Check walk'
                                          : 'Check outcome',
                                  onCheck: _busy ? null : () => _inspect(s)),
                            ],
                            if (_pending) ...[
                              const SizedBox(height: 14),
                              Wrap(spacing: 8, children: [
                                FilledButton(
                                    onPressed: _busy ||
                                            expired ||
                                            steps.indexed.any((x) =>
                                                x.$2['status'] == 'review' &&
                                                !_reviews.containsKey(x.$1))
                                        ? null
                                        : _apply,
                                    child: Text(expired
                                        ? 'Review expired'
                                        : 'Apply reviewed steps')),
                                TextButton(
                                    onPressed: _busy
                                        ? null
                                        : () => _run(() async => _showState(
                                            await widget.client.call('cancel',
                                                {'plan': plan['id']}))),
                                    child: const Text('Skip remaining'))
                              ]),
                            ],
                          ])),
                    if (_busy)
                      AssistantWorking(
                          label: catalog == null
                              ? 'Connecting to your hub…'
                              : 'Working…'),
                  ])),
              if (rooms.isNotEmpty)
                SizedBox(
                    height: 48,
                    child: Row(children: [
                      const Padding(
                          padding: EdgeInsets.only(left: 20, right: 10),
                          child: AssistantNote('Room')),
                      Expanded(
                          child: ListView.separated(
                              scrollDirection: Axis.horizontal,
                              padding: const EdgeInsets.only(right: 16),
                              itemCount: rooms.length,
                              separatorBuilder: (_, __) =>
                                  const SizedBox(width: 8),
                              itemBuilder: (_, i) {
                                final t = rooms[i];
                                return Center(
                                    child: ChoiceChip(
                                        showCheckmark: false,
                                        label: Text(rooms
                                                    .where(
                                                        (v) => v.name == t.name)
                                                    .length >
                                                1
                                            ? '${t.name} (${t.id})'
                                            : t.name),
                                        selected: _focus == t.ref,
                                        onSelected: _busy
                                            ? null
                                            : (selected) => setState(() =>
                                                _focus =
                                                    selected ? t.ref : null)));
                              })),
                    ])),
              if (_photo != null)
                Padding(
                    padding: const EdgeInsets.fromLTRB(16, 6, 6, 0),
                    child: Row(children: [
                      ClipRRect(
                          borderRadius: BorderRadius.circular(8),
                          child: Image.memory(_photo!,
                              height: 44, width: 44, fit: BoxFit.cover)),
                      const SizedBox(width: 12),
                      const Expanded(
                          child: AssistantNote(
                              'Photo ready to send to Cloudflare Workers AI')),
                      IconButton(
                          tooltip: 'Remove photo',
                          color: CelestialColors.textSecondary,
                          onPressed: _busy
                              ? null
                              : () => setState(() => _photo = null),
                          icon: const Icon(Icons.close_rounded)),
                    ])),
              AssistantComposer(
                  controller: _input,
                  hint: _pending
                      ? 'Review the change above'
                      : 'Message about your lights…',
                  sendLabel: 'Send message',
                  maxLength: 1000,
                  canSendEmpty: _photo != null,
                  onSend: canAsk ? _send : null,
                  leading: PopupMenuButton<ImageSource>(
                      tooltip: 'Add a photo',
                      enabled: !_busy && !_pending,
                      icon: const Icon(Icons.add_a_photo_outlined),
                      onSelected: _pick,
                      itemBuilder: (_) => const [
                            PopupMenuItem(
                                value: ImageSource.camera,
                                child: Text('Take photo')),
                            PopupMenuItem(
                                value: ImageSource.gallery,
                                child: Text('Choose photo'))
                          ])),
            ])));
  }
}

/// A target and where its request stands.
class _StepStatus extends StatelessWidget {
  const _StepStatus(
      {required this.title, required this.status, required this.color});
  final String title, status;
  final Color color;

  @override
  Widget build(BuildContext context) =>
      Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
        Text(title, style: Theme.of(context).textTheme.labelLarge),
        Text(status,
            style: TextStyle(color: color, fontSize: 13, height: 1.45)),
      ]);
}

/// One step of the agent's plan inside the review card.
class _AgentStep extends StatelessWidget {
  const _AgentStep(
      {required this.target,
      required this.title,
      required this.operation,
      required this.arguments,
      required this.status,
      required this.statusColor,
      this.detail,
      this.proof,
      this.checkLabel,
      this.onCheck});
  final String target, title, operation, status;
  final CustomerAction arguments;
  final Color statusColor;
  final String? detail, checkLabel;
  final CustomerAction? proof;
  final VoidCallback? onCheck;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
      AssistantReviewHeader(
          target: target,
          title: title,
          operation: operation,
          arguments: arguments),
      const SizedBox(height: 12),
      if (detail != null) Text(detail!),
      Text(status,
          style: TextStyle(color: statusColor, fontSize: 13, height: 1.45)),
      if (proof != null)
        Theme(
            data: theme.copyWith(dividerColor: Colors.transparent),
            child: ExpansionTile(
                tilePadding: EdgeInsets.zero,
                childrenPadding: const EdgeInsets.only(bottom: 8),
                expandedAlignment: Alignment.centerLeft,
                iconColor: CelestialColors.accentBlue,
                collapsedIconColor: CelestialColors.textSecondary,
                title: Text('Review exact changes',
                    style: theme.textTheme.labelLarge),
                children: [
                  SelectableText(
                      const JsonEncoder.withIndent('  ').convert(proof),
                      style: const TextStyle(
                          color: CelestialColors.textSecondary,
                          fontFamily: 'monospace',
                          fontSize: 12,
                          height: 1.4))
                ])),
      if (checkLabel != null)
        TextButton(onPressed: onCheck, child: Text(checkLabel!)),
    ]);
  }
}
