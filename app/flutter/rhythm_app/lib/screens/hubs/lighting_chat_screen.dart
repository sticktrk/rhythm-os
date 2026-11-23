import 'dart:async';

import 'package:dio/dio.dart';
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart' show DirectHubAccess;
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:supabase_flutter/supabase_flutter.dart' show FunctionException;

import '../../providers/home_provider.dart';
import '../../providers/server_sync_provider.dart';
import '../../services/analytics_service.dart';
import '../../services/auth_service.dart';
import '../../services/customer_assistant_service.dart';
import '../../services/lighting_chat_service.dart';
import '../../utils/app_color_temperature.dart';
import '../settings/dialogs/sign_in_modal.dart';
import 'customer_assistant_screen.dart';

class LightingChatScreen extends StatefulWidget {
  const LightingChatScreen(
      {super.key,
      required this.openHub,
      required this.account,
      required this.current,
      required this.changes,
      required this.local,
      required this.guided});
  final Future<LightingChatHub?> Function(bool enroll) openHub;
  final CustomerAssistantService account;
  final bool Function() current;
  final Listenable changes;
  final bool local;
  final VoidCallback guided;

  static Future<void> show(BuildContext context) async {
    final account = CustomerAssistantService.signedIn();
    final user = AuthService().currentUserId;
    if (account == null || user == null) {
      SignInModal.show(context);
      return;
    }
    final homes = context.read<HomeProvider>(),
        sync = context.read<ServerSyncProvider>();
    final hub = homes.activeServerHub, endpoint = sync.activeConnectionEndpoint;
    final instance = sync.connectedServerInstanceId;
    if (hub == null ||
        endpoint == null ||
        instance == null ||
        !sync.connection.connected ||
        sync.connectedServerHub?.id != hub.id ||
        sync.connectedServerHub?.homeId != hub.homeId) {
      await CustomerAssistantScreen.show(context,
          account: account, openHubChat: LightingChatScreen.show);
      return;
    }
    final direct = DirectHubAccess.capture();
    bool current() =>
        account.isCurrentAccount() &&
        direct.isSameSelection &&
        homes.currentHome?.id == hub.homeId &&
        homes.activeServerHub?.id == hub.id &&
        sync.connection.connected &&
        sync.connectedServerHub?.id == hub.id &&
        sync.connectedServerHub?.homeId == hub.homeId &&
        sync.connectedServerInstanceId == instance &&
        sync.activeConnectionEndpoint == endpoint;
    final local = endpoint == hub.endpoint;
    final connection = LightingChatConnection(
        baseUrl: endpoint.baseUrl,
        ownerToken: hub.token,
        local: local,
        accountId: user,
        instance: instance,
        current: current);
    await Navigator.push<void>(
        context,
        MaterialPageRoute(
            builder: (ctx) => LightingChatScreen(
                  openHub: connection.open,
                  current: current,
                  local: local,
                  account: account,
                  changes:
                      Listenable.merge([homes, sync, DirectHubAccess.changes]),
                  guided: () =>
                      CustomerAssistantScreen.show(ctx, account: account),
                )));
  }

  @override
  State<LightingChatScreen> createState() => _LightingChatState();
}

class _LightingChatState extends State<LightingChatScreen> {
  final _input = TextEditingController(), _scroll = ScrollController();
  final _messages = <({bool resident, String text})>[];
  StreamSubscription<void>? _accountSub;
  LightingChatHub? _hub;
  RhythmTopologyNode? _room;
  LightingProposal? _proposal;
  String? _issue, _trial;
  CustomerAction _reading = {};
  bool _busy = false, _ended = false;
  bool get _active =>
      mounted &&
      !_ended &&
      widget.current() &&
      widget.account.isCurrentAccount();

  @override
  void initState() {
    super.initState();
    widget.changes.addListener(_check);
    _accountSub = widget.account.accountChanges.listen((_) => _check());
    _connect(false);
  }

  void _check() {
    if (!mounted || _ended || _active) return;
    setState(() {
      _ended = true;
      _messages.clear();
      _input.clear();
      _proposal = null;
      _hub?.close();
      _hub = null;
    });
  }

  @override
  void dispose() {
    widget.changes.removeListener(_check);
    _accountSub?.cancel();
    _hub?.close();
    _scroll.dispose();
    _input.dispose();
    super.dispose();
  }

  Future<void> _connect(bool enroll) async {
    if (!_active || _busy) return;
    setState(() {
      _busy = true;
      _issue = null;
    });
    try {
      final hub = await widget.openHub(enroll);
      if (!_active) {
        hub?.close();
        return;
      }
      setState(() {
        _hub = hub;
        if (hub?.rooms.length == 1) _room = hub!.rooms.single;
      });
    } catch (error) {
      if (_active) {
        setState(() => _issue = switch (error) {
              DioException(response: final r) when r?.statusCode == 403 => widget
                      .local
                  ? 'Pair this phone locally using the hub owner account.'
                  : 'Remote MCP must be enabled on this hub to chat away from home.',
              DioException(response: final r) when r?.statusCode == 401 =>
                'Chat access has expired. Reconnect locally to pair this phone.',
              _ =>
                'Could not open lighting chat on this hub. Check its connection and MCP support, or use Guided tools.',
            });
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  void _append(String text, {bool resident = false}) {
    _messages.add((resident: resident, text: text));
    _showLatest();
  }

  void _showLatest() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && _scroll.hasClients) {
        _scroll.jumpTo(_scroll.position.maxScrollExtent);
      }
    });
  }

  Future<void> _send(String message) async {
    final hub = _hub, room = _room, prompt = message.trim();
    if (!_active ||
        _busy ||
        hub == null ||
        room == null ||
        prompt.isEmpty ||
        prompt.length > 1000 ||
        _proposal != null) {
      return;
    }
    setState(() {
      _input.clear();
      _busy = true;
      _append(prompt, resident: true);
    });
    var outcome = LightingChatOutcome.hubUnavailable;
    try {
      final current =
          await hub.call('explain_room_output', {'room_id': room.id});
      if (!_active) return;
      if (current['room_id'] != room.id) throw StateError('Room changed');
      outcome = LightingChatOutcome.modelUnavailable;
      final result = await widget.account.lightingChat({
        'messages': [
          for (final m in _messages
              .skip(_messages.length > 8 ? _messages.length - 8 : 0))
            {'role': m.resident ? 'user' : 'assistant', 'text': m.text}
        ],
        'tools': hub.tools.toList(),
        'room': lightingReading(current),
      });
      if (!_active) return;
      if (result['operation'] != null) {
        final proposal = LightingProposal.parse(result, hub.tools);
        setState(() {
          _proposal = proposal;
          _reading = lightingReading(current);
        });
        _showLatest();
        outcome = LightingChatOutcome.proposal;
      } else {
        final text = result['text'];
        if (text is! String || text.trim().isEmpty || text.length > 1000) {
          throw const FormatException();
        }
        setState(() => _append(text));
        outcome = LightingChatOutcome.reply;
      }
    } catch (e) {
      if (_active) {
        setState(() => _append(switch (e) {
              FunctionException(status: 429) =>
                'You’ve reached today’s chat preview limit. Guided tools are still available.',
              _ when outcome == LightingChatOutcome.hubUnavailable =>
                'Could not read this room. Check the hub connection and try again.',
              _ =>
                'The AI service is unavailable. Try again or use Guided tools.',
            }));
      }
    } finally {
      if (mounted) setState(() => _busy = false);
      unawaited(AnalyticsService().logLightingChatTurn(outcome));
    }
  }

  Future<void> _apply({bool undo = false}) async {
    final room = _room, hub = _hub, proposal = _proposal, trial = _trial;
    if (!_active ||
        _busy ||
        room == null ||
        hub == null ||
        (undo ? trial == null : proposal == null)) {
      return;
    }
    setState(() {
      _busy = true;
      _proposal = null;
      _trial = null;
      _append(undo ? 'Undo preview' : 'Apply ${proposal!.label}',
          resident: true);
    });
    bool sent = false;
    try {
      // Revalidate the signed-in account before dispatching a reviewed change.
      await widget.account.verifyChatSession();
      if (!_active) return;
      sent = true;
      final result =
          await hub.call(undo ? 'undo_room_trial' : proposal!.operation, {
        'room_id': room.id,
        if (undo) 'trial_id': trial else ...proposal!.arguments,
      });
      if (!_active) return;
      final skippedRestore = undo &&
          result['restored'] == false &&
          const {
            'skipped_missing_target',
            'skipped_paused_target',
            'skipped_changed_ownership'
          }.contains(result['restoration']);
      if (result['room_id'] != room.id ||
          (result['server_acknowledged'] != true && !skippedRestore)) {
        throw StateError('Unverified receipt');
      }
      final trialId = result['trial_id'];
      if (!undo && trialId is String && hub.canUndo) _trial = trialId;
      final lights = (result['light_readback'] as List? ?? const [])
          .whereType<Map>()
          .where((l) => l['control_scope'] != 'outside_ha_selection');
      final fresh = lights
          .where((l) =>
              l['verification'] == 'fresh_device_readback' &&
              l['observed'] is Map)
          .length;
      final target = [
        if (result['canonical_brightness'] is num)
          "${result['canonical_brightness']}%",
        if (result['canonical_kelvin'] is num)
          "${result['canonical_kelvin']} K",
      ].join(' · ');
      final receipt = [
        undo
            ? (result['restored'] == false
                ? 'Preview ended. Restoration was skipped.'
                : 'The hub ended the preview.')
            : 'The hub accepted the adjustment.',
        if (target.isNotEmpty) 'Room target $target.',
        fresh == 0
            ? 'No fresh lamp readings yet.'
            : 'Fresh readings from $fresh of ${lights.length} managed lights.',
      ].join(' ');
      setState(() => _append(receipt));
    } catch (_) {
      if (_active) {
        setState(() => _append(sent
            ? 'The result is uncertain. Ask what the lights are doing to read the room again.'
            : 'Sign in again to make changes. The request stopped before reaching the hub.'));
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context), colors = theme.colorScheme;
    final hub = _hub, room = _room, proposal = _proposal;
    final enabled = _active && !_busy;
    final suggestions = <(IconData, String, String)>[
      (
        Icons.lightbulb_outline,
        'Check the lights',
        'What are my lights doing right now?'
      ),
      if (hub?.tools.contains('adjust_room_brightness') == true)
        (
          Icons.brightness_6_outlined,
          'A little brighter',
          'Make this room a little brighter.'
        ),
      if (hub?.tools.contains('adjust_room_warmth') == true)
        (
          Icons.wb_twilight,
          'Warm it up',
          'Make the light a little warmer and keep the brightness.'
        ),
      if (hub?.tools.contains('set_room_color') == true)
        (
          Icons.palette_outlined,
          'Try a color',
          'Suggest a one-minute amber color preview.'
        ),
    ];
    return Scaffold(
        appBar: AppBar(title: const Text('Lighting assistant'), actions: [
          if (_active)
            IconButton(
                tooltip: 'Guided tools',
                onPressed: _busy ? null : widget.guided,
                icon: const Icon(Icons.tune)),
        ]),
        body: SafeArea(
            child: !_active
                ? const Center(
                    child: Padding(
                        padding: EdgeInsets.all(24),
                        child: Text(
                            'Your account or hub changed. Open a new chat to continue.')))
                : Center(
                    child: ConstrainedBox(
                        constraints: const BoxConstraints(maxWidth: 640),
                        child: Column(children: [
                          if (_busy) const LinearProgressIndicator(),
                          if (hub != null && hub.rooms.isNotEmpty)
                            Padding(
                                padding: const EdgeInsets.symmetric(
                                    horizontal: 16, vertical: 6),
                                child: DropdownButtonFormField<String>(
                                  initialValue: room?.id,
                                  isExpanded: true,
                                  decoration: const InputDecoration(
                                      labelText: 'Room',
                                      prefixIcon:
                                          Icon(Icons.meeting_room_outlined)),
                                  items: [
                                    for (final r in hub.rooms)
                                      DropdownMenuItem(
                                          value: r.id,
                                          child: Text(
                                              r.name.isEmpty ? r.id : r.name,
                                              overflow: TextOverflow.ellipsis))
                                  ],
                                  onChanged: !enabled
                                      ? null
                                      : (id) => setState(() {
                                            if (_room?.id == id) return;
                                            _room = hub.rooms
                                                .firstWhere((r) => r.id == id);
                                            _messages.clear();
                                            _input.clear();
                                            _proposal = null;
                                            _trial = null;
                                          }),
                                )),
                          Expanded(
                              child: ListView(
                                  controller: _scroll,
                                  padding: const EdgeInsets.all(16),
                                  children: [
                                if (hub == null && !_busy) ...[
                                  Text(
                                      _issue ??
                                          (widget.local
                                              ? 'Connect lighting chat to this hub'
                                              : 'Pair chat at home first'),
                                      style: theme.textTheme.titleLarge),
                                  const SizedBox(height: 12),
                                  Text(widget.local
                                      ? 'Chat sends your messages and room lighting readings to OpenAI. This phone uses a revocable credential to control the lights. You can manage access in the hub’s Assistant access settings.'
                                      : 'Open this screen on the hub’s local network to connect this phone. Away from home, chat uses your enabled remote MCP connection.'),
                                  const SizedBox(height: 16),
                                  if (_issue != null || widget.local)
                                    FilledButton(
                                        onPressed: enabled
                                            ? () => _connect(_issue == null)
                                            : null,
                                        child: Text(_issue == null
                                            ? 'Connect lighting chat'
                                            : 'Check connection')),
                                  TextButton(
                                      onPressed: widget.guided,
                                      child: const Text('Open guided tools')),
                                ],
                                if (hub != null && _messages.isEmpty) ...[
                                  Text(
                                      room == null
                                          ? (hub.rooms.isEmpty
                                              ? 'No rooms found on this hub'
                                              : 'Choose a room to start')
                                          : 'How should the light feel?',
                                      style: theme.textTheme.headlineSmall),
                                  const SizedBox(height: 8),
                                  const Text(
                                      'Your messages and the selected room’s lighting readings are sent to OpenAI. You review each adjustment before it reaches your lights.'),
                                  if (room != null)
                                    Padding(
                                        padding: const EdgeInsets.only(top: 16),
                                        child: Wrap(
                                            spacing: 8,
                                            runSpacing: 8,
                                            children: [
                                              for (final s in suggestions)
                                                ActionChip(
                                                    avatar:
                                                        Icon(s.$1, size: 18),
                                                    label: Text(s.$2),
                                                    onPressed: enabled
                                                        ? () => _send(s.$3)
                                                        : null),
                                            ])),
                                ],
                                for (final m in _messages)
                                  Align(
                                      alignment: m.resident
                                          ? Alignment.centerRight
                                          : Alignment.centerLeft,
                                      child: Container(
                                        constraints:
                                            const BoxConstraints(maxWidth: 480),
                                        margin:
                                            const EdgeInsets.only(bottom: 10),
                                        padding: const EdgeInsets.symmetric(
                                            horizontal: 14, vertical: 10),
                                        decoration: BoxDecoration(
                                            color: m.resident
                                                ? colors.secondaryContainer
                                                : colors.surfaceContainerHigh,
                                            borderRadius:
                                                BorderRadius.circular(16)),
                                        child: SelectableText(m.text,
                                            style: TextStyle(
                                                color: m.resident
                                                    ? colors
                                                        .onSecondaryContainer
                                                    : colors.onSurface)),
                                      )),
                                if (proposal != null)
                                  _LightingReview(
                                      proposal: proposal,
                                      roomName: room!.name.isEmpty
                                          ? room.id
                                          : room.name,
                                      reading: _reading,
                                      apply: enabled ? () => _apply() : null,
                                      skip: enabled
                                          ? () => setState(() {
                                                _proposal = null;
                                                _append(
                                                    'Skipped ${proposal.label}.');
                                              })
                                          : null),
                                if (_trial != null && proposal == null)
                                  Align(
                                      alignment: Alignment.centerLeft,
                                      child: ActionChip(
                                          label: const Text('Undo preview'),
                                          onPressed: enabled
                                              ? () => _apply(undo: true)
                                              : null)),
                              ])),
                          if (room != null)
                            Padding(
                                padding:
                                    const EdgeInsets.fromLTRB(12, 0, 12, 12),
                                child: TextField(
                                  controller: _input,
                                  enabled: enabled && proposal == null,
                                  maxLength: 1000,
                                  textInputAction: TextInputAction.send,
                                  onSubmitted: _send,
                                  decoration: InputDecoration(
                                      hintText: 'Ask about this room…',
                                      counterText: '',
                                      suffixIcon: IconButton(
                                          tooltip: 'Send message',
                                          icon: const Icon(Icons.arrow_upward),
                                          onPressed: enabled && proposal == null
                                              ? () => _send(_input.text)
                                              : null)),
                                )),
                        ])))));
  }
}

class _LightingReview extends StatelessWidget {
  const _LightingReview(
      {required this.proposal,
      required this.roomName,
      required this.reading,
      this.apply,
      this.skip});
  final LightingProposal proposal;
  final String roomName;
  final CustomerAction reading;
  final VoidCallback? apply, skip;
  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context), args = proposal.arguments;
    final swatch = switch (proposal.operation) {
      'set_room_color' =>
        Color.fromRGBO(args['red'], args['green'], args['blue'], 1),
      'adjust_room_warmth' => AppColorTemperature.toColor(args['kelvin']),
      _ => null
    };
    final brightness = reading['canonical_brightness'],
        kelvin = reading['canonical_kelvin'];
    final before = switch (proposal.operation) {
      'adjust_room_warmth' when kelvin is num =>
        'Current target ${kelvin.round()} K',
      'adjust_room_brightness' when brightness is num =>
        'Current target ${brightness.round()}%',
      _ => null
    };
    return Card(
        child: Padding(
            padding: const EdgeInsets.all(16),
            child:
                Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
              Row(children: [
                CircleAvatar(
                    backgroundColor:
                        swatch ?? theme.colorScheme.secondaryContainer,
                    child: swatch == null
                        ? Icon(proposal.operation == 'try_room_offset'
                            ? Icons.schedule
                            : Icons.lightbulb_outline)
                        : null),
                const SizedBox(width: 12),
                Expanded(
                    child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                      Text(roomName, style: theme.textTheme.labelLarge),
                      Text(proposal.label, style: theme.textTheme.titleLarge),
                    ])),
              ]),
              const SizedBox(height: 12),
              if (before != null)
                Text(before, style: theme.textTheme.labelMedium),
              Text(proposal.detail),
              const SizedBox(height: 12),
              Wrap(spacing: 8, children: [
                FilledButton(
                    onPressed: apply, child: const Text('Apply to room')),
                TextButton(onPressed: skip, child: const Text('Skip')),
              ]),
            ])));
  }
}
