import 'dart:async';

import 'package:dio/dio.dart';
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart' show DirectHubAccess;
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:supabase_flutter/supabase_flutter.dart' show FunctionException;

import '../../providers/home_provider.dart';
import '../../providers/room_provider.dart';
import '../../providers/server_sync_provider.dart';
import '../../services/analytics_service.dart';
import '../../services/auth_service.dart';
import '../../services/customer_assistant_service.dart';
import '../../services/lighting_chat_service.dart';
import '../../utils/app_color_temperature.dart';
import '../settings/dialogs/sign_in_modal.dart';
import '../../widgets/assistant_chat_widgets.dart';
import '../../widgets/connect_hub_screen.dart';
import '../../widgets/solar_orbit.dart' show CelestialColors;
import 'assistant_chat_screen.dart';

class LightingChatScreen extends StatefulWidget {
  const LightingChatScreen(
      {super.key,
      required this.openHub,
      required this.account,
      required this.current,
      required this.changes,
      required this.local,
      required this.guided,
      this.pairPhone,
      this.initialRoomId});
  final Future<LightingChatHub> Function({bool reconnect}) openHub;
  final CustomerAssistantService account;
  final bool Function() current;
  final Listenable changes;
  final bool local;
  final VoidCallback guided;
  final String? initialRoomId;
  final Future<void> Function()? pairPhone;

  static Future<void> show(BuildContext context) async {
    if (CustomerAssistantService.signedIn() == null) {
      await SignInModal.show(context, returnToCaller: true);
      if (!context.mounted) return;
    }
    final account = CustomerAssistantService.signedIn();
    final user = AuthService().currentUserId;
    if (account == null || user == null) return;

    final homes = context.read<HomeProvider>(),
        sync = context.read<ServerSyncProvider>();
    bool connected() {
      final hub = homes.activeServerHub;
      return hub != null &&
          sync.connection.connected &&
          sync.connectedServerHub?.id == hub.id &&
          sync.connectedServerHub?.homeId == hub.homeId &&
          sync.activeConnectionEndpoint != null &&
          sync.connectedServerInstanceId != null;
    }

    if (!connected()) {
      await ConnectHubScreen.show(context);
      if (!context.mounted || !account.isCurrentAccount() || !connected()) {
        return;
      }
    }
    final hub = homes.activeServerHub!,
        endpoint = sync.activeConnectionEndpoint!;
    final instance = sync.connectedServerInstanceId!;
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
    final initialRoomId = context.read<RoomProvider>().currentRoom?.id;
    await Navigator.push<void>(
        context,
        MaterialPageRoute(
            builder: (ctx) => LightingChatScreen(
                  openHub: ({bool reconnect = false}) => LightingChatConnection(
                    baseUrl: endpoint.baseUrl,
                    ownerToken: homes.activeServerHub?.token,
                    local: local,
                    accountId: user,
                    instance: instance,
                    current: () => ctx.mounted && current(),
                  ).open(reconnect: reconnect),
                  initialRoomId: initialRoomId,
                  pairPhone: () => ConnectHubScreen.show(ctx),
                  current: current,
                  local: local,
                  account: account,
                  changes:
                      Listenable.merge([homes, sync, DirectHubAccess.changes]),
                  guided: () => AssistantChatScreen.show(ctx, account: account),
                )));
  }

  @override
  State<LightingChatScreen> createState() => _LightingChatState();
}

class _LightingChatState extends State<LightingChatScreen> {
  final _input = TextEditingController(), _scroll = ScrollController();
  final _messages = <({bool resident, String text, bool share})>[];
  List<RhythmTopologyNode> _choices = [];
  StreamSubscription<void>? _accountSub;
  LightingChatHub? _hub;
  RhythmTopologyNode? _room;
  LightingProposal? _proposal;
  Object? _issue;
  String? _trial;
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
    _connect();
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

  Future<void> _connect({bool reconnect = false}) async {
    if (!_active || _busy) return;
    setState(() {
      _busy = true;
      _issue = null;
    });
    try {
      final hub = await widget.openHub(reconnect: reconnect);
      if (!_active) {
        hub.close();
        return;
      }
      setState(() {
        _hub = hub;
        _room = hub.rooms.length == 1
            ? hub.rooms.single
            : hub.rooms.where((r) => r.id == widget.initialRoomId).firstOrNull;
      });
    } catch (error) {
      if (_active) setState(() => _issue = error);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  bool _lostAccess(Object error) {
    if (error is! DioException) return false;
    final status = error.response?.statusCode;
    if (status != 401 && !(status == 403 && !widget.local)) return false;
    setState(() {
      _issue = status == 401 ? LightingChatAccessIssue.revoked : error;
      _hub?.close();
      _hub = null;
      _room = null;
      _proposal = null;
      _trial = null;
      _choices.clear();
      _messages.clear();
      _input.clear();
    });
    return true;
  }

  void _append(String text, {bool resident = false, bool share = true}) {
    _messages.add((resident: resident, text: text, share: share));
    _showLatest();
  }

  void _showLatest() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && _scroll.hasClients) {
        _scroll.jumpTo(_scroll.position.maxScrollExtent);
      }
    });
  }

  String _label(RhythmTopologyNode room) {
    if (room.name.isEmpty) return room.id;
    final duplicate =
        _hub!.rooms.any((r) => r.id != room.id && r.name == room.name);
    return duplicate ? '${room.name} (${room.id})' : room.name;
  }

  void _askRoom(List<RhythmTopologyNode> rooms) {
    _choices = rooms;
    _append(
        rooms.isEmpty
            ? 'Your hub has no rooms yet. Add a room to control its lights.'
            : 'Which room should I use for that?',
        share: false);
  }

  Future<void> _send(String message, {RhythmTopologyNode? choice}) async {
    final hub = _hub, prompt = message.trim();
    if (!_active ||
        _busy ||
        hub == null ||
        prompt.isEmpty ||
        (choice == null && prompt.length > 1000) ||
        _proposal != null) {
      return;
    }
    final matches = hub.mentionedRooms(prompt);
    // A typed room answer resumes the outstanding request just like a tap.
    if (choice == null && _choices.isNotEmpty && matches.length == 1) {
      final answer = LightingChatHub.words(prompt)
          .replaceFirst(RegExp(r'^the '), '')
          .replaceFirst(RegExp(r' please$'), '');
      if (_choices.contains(matches.single) &&
          (answer == LightingChatHub.words(matches.single.name) ||
              LightingChatHub.words(prompt) ==
                  LightingChatHub.words(matches.single.name))) {
        choice = matches.single;
      }
    }
    final room =
        choice ?? (matches.length > 1 ? null : matches.firstOrNull ?? _room);
    final scope = choice != null
        ? 'clarified'
        : matches.length == 1
            ? 'mentioned'
            : room == null
                ? 'unresolved'
                : 'continuing';
    setState(() {
      _input.clear();
      _choices = [];
      _append(prompt, resident: true, share: choice == null);
      if (room != null && room.id != _room?.id) {
        _trial = null;
        _room = room;
      }
      _busy = true;
    });
    var outcome = LightingChatOutcome.hubUnavailable;
    try {
      CustomerAction current = {};
      if (room != null) {
        current = await hub.call('explain_room_output', {'room_id': room.id});
        if (!_active) return;
        if (current['room_id'] != room.id) throw StateError('Room changed');
      }
      outcome = LightingChatOutcome.modelUnavailable;
      final history = _messages.where((m) => m.share).toList();
      final result = await widget.account.lightingChat({
        'messages': [
          for (final m
              in history.skip(history.length > 8 ? history.length - 8 : 0))
            {'role': m.resident ? 'user' : 'assistant', 'text': m.text}
        ],
        'scope': scope,
        'tools': room == null ? <String>[] : hub.tools.toList(),
        'room': lightingReading(current),
      });
      if (!_active) return;
      if (result['rooms'] == 'clarify') {
        setState(() => _askRoom(matches.length > 1 ? matches : hub.rooms));
        outcome = LightingChatOutcome.reply;
      } else if (result['rooms'] == 'list') {
        setState(() => _append(
            hub.rooms.isEmpty
                ? 'Your hub has no rooms yet.'
                : 'Your rooms are ${hub.rooms.map(_label).join(', ')}.',
            share: false));
        outcome = LightingChatOutcome.reply;
      } else if (result['operation'] != null) {
        if (room == null) {
          throw const FormatException(
              'A lighting change requires a resolved room');
        }
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
        if (outcome == LightingChatOutcome.hubUnavailable && _lostAccess(e)) {
          return;
        }
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
    } catch (error) {
      if (_active) {
        if (sent && _lostAccess(error)) return;
        setState(() => _append(sent
            ? 'The result is uncertain. Ask what the lights are doing to read the room again.'
            : 'Sign in again to make changes. The request stopped before reaching the hub.'));
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Widget _connectionHelp() {
    final (message, action, callback) = switch (_issue) {
      LightingChatAccessIssue.owner => (
          'Pair this phone as the hub owner on your home Wi-Fi to start chatting.',
          'Pair this phone',
          widget.pairPhone == null
              ? null
              : () async {
                  await widget.pairPhone!();
                  if (_active) await _connect();
                }
        ),
      LightingChatAccessIssue.revoked when widget.local => (
          'Chat access for this phone has ended. Reconnect to use it again.',
          'Reconnect chat',
          () => _connect(reconnect: true)
        ),
      LightingChatAccessIssue.localNetwork ||
      LightingChatAccessIssue.revoked =>
        (
          'Open chat on your home Wi-Fi to connect this phone to your hub.',
          'Check connection',
          () => _connect()
        ),
      DioException(response: final r)
          when r?.statusCode == 403 && !widget.local =>
        (
          'To chat away from home, turn on Remote MCP in Rhythm Pro on your home Wi-Fi.',
          'Check connection',
          () => _connect()
        ),
      UnsupportedError() ||
      FormatException() ||
      DioException(response: Response(statusCode: 404)) =>
        (
          'Update your hub to use lighting chat. Guided tools are available now.',
          'Open guided tools',
          widget.guided
        ),
      _ => (
          'Your hub couldn’t open chat. Check that it’s online, then try again.',
          'Try again',
          () => _connect()
        ),
    };
    return Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
      AssistantMessageBubble(resident: false, text: message),
      OutlinedButton(onPressed: callback, child: Text(action)),
    ]);
  }

  @override
  Widget build(BuildContext context) {
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
          'Make the lights a little brighter.'
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
    return RhythmAssistantPage(
      title: 'Lighting assistant',
      actions: [
        if (_active)
          PopupMenuButton<void Function()>(
            tooltip: 'More options',
            enabled: !_busy,
            icon: const Icon(Icons.more_horiz),
            onSelected: (action) => action(),
            itemBuilder: (_) => [
              PopupMenuItem(
                  value: widget.guided, child: const Text('Guided tools'))
            ],
          ),
      ],
      body: !_active
          ? const Center(
              child: Padding(
                  padding: EdgeInsets.all(24),
                  child: Text(
                      'Your account or hub changed. Open a new chat to continue.')))
          : Column(children: [
              Expanded(
                child: ListView(
                    controller: _scroll,
                    padding: const EdgeInsets.fromLTRB(20, 16, 20, 8),
                    children: [
                      if (hub == null && !_busy) _connectionHelp(),
                      if (hub != null && _messages.isEmpty) ...[
                        SizedBox(
                            height: (MediaQuery.sizeOf(context).height * 0.12 -
                                    MediaQuery.viewInsetsOf(context).bottom)
                                .clamp(0.0, 96.0)),
                        const Text('How should the light feel?',
                            style: TextStyle(
                                fontSize: 24,
                                fontWeight: FontWeight.w600,
                                height: 1.2,
                                color: CelestialColors.textPrimary)),
                        const SizedBox(height: 14),
                        Wrap(spacing: 8, runSpacing: 4, children: [
                          for (final s in suggestions)
                            ActionChip(
                                avatar: Icon(s.$1),
                                label: Text(s.$2),
                                onPressed: enabled ? () => _send(s.$3) : null),
                        ]),
                        const SizedBox(height: 14),
                        const Text(
                            'Chat shares your messages and room readings with OpenAI. You review each lighting change.',
                            style: TextStyle(
                                color: CelestialColors.textSecondary,
                                fontSize: 13,
                                height: 1.45)),
                      ],
                      for (final m in _messages)
                        AssistantMessageBubble(
                            resident: m.resident, text: m.text),
                      if (_choices.isNotEmpty)
                        Wrap(spacing: 8, runSpacing: 4, children: [
                          for (final r in _choices)
                            ActionChip(
                                label: Text(_label(r)),
                                onPressed: enabled
                                    ? () => _send(_label(r), choice: r)
                                    : null),
                        ]),
                      if (_busy)
                        Row(children: [
                          const SizedBox.square(
                              dimension: 16,
                              child: CircularProgressIndicator(strokeWidth: 2)),
                          const SizedBox(width: 10),
                          Expanded(
                              child: Semantics(
                                  liveRegion: true,
                                  child: Text(hub == null
                                      ? 'Connecting to your hub…'
                                      : 'Working…'))),
                        ]),
                      if (proposal != null)
                        _LightingReview(
                            proposal: proposal,
                            roomName: _label(room!),
                            reading: _reading,
                            apply: enabled ? () => _apply() : null,
                            skip: enabled
                                ? () => setState(() {
                                      _proposal = null;
                                      _append('Skipped ${proposal.label}.');
                                    })
                                : null),
                      if (_trial != null && proposal == null)
                        Align(
                            alignment: Alignment.centerLeft,
                            child: ActionChip(
                                avatar: const Icon(Icons.undo_rounded),
                                label: const Text('Undo preview'),
                                onPressed:
                                    enabled ? () => _apply(undo: true) : null)),
                    ]),
              ),
              if (hub != null)
                AssistantComposer(
                    controller: _input,
                    maxLength: 1000,
                    sendLabel: 'Send message',
                    hint: proposal != null
                        ? 'Review the change above'
                        : 'Message about your lights…',
                    onSend: enabled && proposal == null ? _send : null),
            ]),
    );
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
        color: CelestialColors.backgroundCard,
        margin: EdgeInsets.zero,
        elevation: 0,
        shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(16),
            side: const BorderSide(color: CelestialColors.orbitRing)),
        child: Padding(
            padding: const EdgeInsets.all(16),
            child:
                Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
              Row(children: [
                CircleAvatar(
                    backgroundColor: swatch ??
                        CelestialColors.sunWarm.withValues(alpha: 0.18),
                    child: swatch == null
                        ? Icon(
                            color: CelestialColors.sunWarm,
                            proposal.operation == 'try_room_offset'
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
