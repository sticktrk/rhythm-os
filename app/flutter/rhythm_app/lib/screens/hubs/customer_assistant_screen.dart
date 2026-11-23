import 'dart:async';

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../../providers/home_provider.dart';
import '../../providers/server_sync_provider.dart';
import '../../services/customer_assistant_service.dart';
import 'assistant_chat_screen.dart';
import '../settings/dialogs/sign_in_modal.dart';

/// The account side of the resident conversation. Messages live on the phone,
/// including when control passes to the selected Box's guided tools.
class CustomerAssistantScreen extends StatefulWidget {
  const CustomerAssistantScreen({
    super.key,
    required this.service,
    required this.device,
    required this.continueOnHub,
    this.history,
  });
  final CustomerAssistantService service;
  final ResidentAssistantDevice device;
  final List<AssistantMessage>? history;
  final Future<void> Function(BuildContext, List<AssistantMessage>)
      continueOnHub;

  static Future<void> show(BuildContext context,
      {List<AssistantMessage>? history,
      CustomerAssistantService? account,
      Future<void> Function(BuildContext)? openHubChat}) async {
    final service = account ?? CustomerAssistantService.signedIn();
    if (service == null) {
      SignInModal.show(context);
      return;
    }
    if (!service.isCurrentAccount()) return;
    final device = ResidentAssistantDevice(context.read<HomeProvider>(),
        context.read<ServerSyncProvider>(), service.isCurrentAccount);
    await Navigator.push<void>(
        context,
        MaterialPageRoute(
            builder: (_) => CustomerAssistantScreen(
                  service: service,
                  device: device,
                  history: history,
                  continueOnHub: (context, history) => openHubChat != null
                      ? openHubChat(context)
                      : AssistantChatScreen.show(context,
                          history: history, account: service),
                )));
  }

  @override
  State<CustomerAssistantScreen> createState() => _CustomerAssistantState();
}

enum _Choice { homes, hubs, members, pending }

class _CustomerAssistantState extends State<CustomerAssistantScreen> {
  late final List<AssistantMessage> _history = widget.history ?? [];
  final _text = TextEditingController();
  final _scroll = ScrollController();
  RhythmAssistantContract? _contract;
  bool _busy = false;
  StreamSubscription<void>? _accountSub;
  String? _home, _hub, _instance;
  CustomerAction? _review;
  _Choice? _choice;
  var _items = <CustomerAction>[];
  String? _intent;
  bool _includeBundle = true;
  bool _accountEnded = false;
  bool get _active =>
      mounted && !_accountEnded && widget.service.isCurrentAccount();
  bool _offers(String id) => _contract?.supportsOperation(id) == true;

  @override
  void initState() {
    super.initState();
    _accountSub = widget.service.accountChanges.listen((_) {
      if (!mounted) return;
      if (!widget.service.isCurrentAccount()) {
        _accountEnded = true;
        _review = null;
        _text.clear();
        _history.clear();
        final route = ModalRoute.of(context);
        if (route != null && route.isActive) {
          Navigator.of(context).popUntil((r) => r == route);
        }
      }
      setState(() {});
    });
    unawaited(_run(_load));
  }

  @override
  void dispose() {
    _accountSub?.cancel();
    _text.dispose();
    _scroll.dispose();
    super.dispose();
  }

  void _say(String text, {bool resident = false}) {
    if (!_active) return;
    setState(() => _history.add((text: text, resident: resident)));
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && _scroll.hasClients) {
        _scroll.jumpTo(_scroll.position.maxScrollExtent);
      }
    });
  }

  Future<void> _run(Future<void> Function() action) async {
    if (!_active || _busy) return;
    setState(() => _busy = true);
    try {
      await action();
    } catch (_) {
      if (_active) {
        _review = null;
        _say(
            'I could not verify that request. Check your account and connection to the selected Box. You can check pending actions before trying again.');
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _load() async {
    _contract = await widget.service.discover();
    _say(
        'Choose a Home, check who has access, or prepare a request for support.');
  }

  Future<CustomerAction> _call(String id, [CustomerAction args = const {}]) {
    if (!_active || !_offers(id)) throw StateError('Tool unavailable');
    return widget.service.call(id, args);
  }

  Future<void> _list(_Choice choice) async {
    final (op, key) = switch (choice) {
      _Choice.homes => ('list_homes', 'homes'),
      _Choice.hubs => ('list_home_hubs', 'hubs'),
      _Choice.members => ('list_home_members', 'members'),
      _Choice.pending => ('list_pending_actions', 'actions'),
    };
    final result = await _call(
        op,
        [_Choice.hubs, _Choice.members].contains(choice)
            ? {'home_id': _home}
            : {});
    if (!_active) return;
    _choice = choice;
    _items = (result[key] as List)
        .map((row) => CustomerAction.from(row as Map))
        .toList();
    final rows = _items.map((row) {
      final label = _label(row);
      return choice == _Choice.homes &&
              row['home_id'] == widget.device.connectedHomeId
          ? '$label · currently connected'
          : label;
    }).join('\n');
    _say(_items.isEmpty ? 'There are no ${choice.name} to show.' : rows);
  }

  String _label(CustomerAction row) {
    return switch (_choice) {
      _Choice.homes => (row['display_name'] ?? row['home_id']).toString(),
      _Choice.hubs => (row['display_name'] ?? row['hub_id']).toString(),
      _Choice.members =>
        "${row['is_self'] == true ? 'You' : row['display_name'] ?? row['member_id']} · ${row['role']}",
      _Choice.pending =>
        '${row['change'].toString().replaceAll('_', ' ')} · ${row['status']}',
      _ => '',
    };
  }

  Future<void> _pick(CustomerAction row) async {
    _say(_label(row), resident: true);
    switch (_choice) {
      case _Choice.homes:
        _home = row['home_id'] as String;
        _hub = null;
        _instance = null;
        _choice = null;
        _items = [];
        final next = _intent;
        if (next == 'remove_member' || next == 'members') {
          await _list(_Choice.members);
        } else {
          await _list(_Choice.hubs);
        }
      case _Choice.hubs:
        final handoff = await _call(
            'select_hub', {'home_id': _home, 'hub_id': row['hub_id']});
        if (handoff['status'] != 'handoff' ||
            handoff['actor'] != 'resident_app' ||
            handoff['home_id'] != _home ||
            handoff['hub_id'] != row['hub_id']) {
          throw StateError('Invalid selection');
        }
        _hub = handoff['hub_id'] as String;
        _instance = handoff['server_instance_id'] as String;
        _choice = null;
        _items = [];
        _say('Selected ${row['display_name'] ?? _hub}.');
        final intent = _intent;
        if (intent != null && !{'members', 'remove_member'}.contains(intent)) {
          await _change(intent);
        }
      case _Choice.members:
        if (_intent == 'remove_member') {
          await _plan('remove_member',
              {'home_id': _home, 'member_id': row['member_id']});
        }
      case _Choice.pending:
        _items = [];
        _choice = null;
        if (row['status'] == 'claimed') {
          _say(
              'A device has already claimed this action. Its result is uncertain. Check the current state before preparing a new change.');
        } else {
          _showReview(row);
        }
      case null:
        break;
    }
  }

  Future<void> _change(String change) async {
    _intent = change;
    if (change == 'join_home') {
      await _plan(change, await widget.device.joinTarget());
      return;
    }
    if (_home == null) {
      await _list(_Choice.homes);
      return;
    }
    if (change == 'remove_member') {
      await _list(_Choice.members);
      return;
    }
    if (_hub == null) {
      await _list(_Choice.hubs);
      return;
    }
    if (change == 'submit_report') {
      _say(
          'What happened? Enter the summary to send to support, and choose whether to attach diagnostics.');
      _text.clear();
      return;
    }
    await _plan(change, {'home_id': _home, 'hub_id': _hub});
  }

  Future<void> _plan(String change, CustomerAction args) async {
    final review =
        await _call('plan_customer_change', {'change': change, ...args});
    if (review['status'] != 'review' ||
        review['change'] != change ||
        args.keys.any((key) => review[key] != args[key])) {
      throw StateError('The review does not match this request');
    }
    _showReview(review);
  }

  void _showReview(CustomerAction review) {
    if (!{'review', 'awaiting_device'}.contains(review['status']) ||
        review['plan_id'] is! String ||
        review['change'] is! String) {
      throw StateError('Missing review');
    }
    _review = review;
    _intent = null;
    _choice = null;
    _items = [];
    _say([
      review['description']?.toString() ??
          review['change'].toString().replaceAll('_', ' '),
      for (final key in [
        'home_id',
        'hub_id',
        'server_instance_id',
        'member_id',
        'report_summary',
        'expires_at'
      ])
        if (review[key] != null) '${key.replaceAll('_', ' ')} · ${review[key]}',
      if (review['change'] == 'grant_support')
        'Join this Box’s local network before granting support access.',
      if (review['support_scope'] != null)
        'Support permission · ${review['support_scope']}',
      if (review['previous_grant_active'] is bool)
        'Support access currently ${review['previous_grant_active'] == true ? 'enabled' : 'off'}',
      if (review['change'] == 'submit_report') ...[
        'Includes your account email, app version, build, and platform.',
        review['include_bundle'] == true
            ? 'Attach Box and app logs, Box name and connection details, and system information.'
            : 'Send the summary without log attachments.'
      ],
      'Confirm this exact request or cancel.'
    ].join('\n'));
  }

  Future<void> _confirm() async {
    final reviewed = _review!;
    _review = null;
    final result = await widget.service.confirm(reviewed,
        checkDevice: widget.device.check, executeDevice: widget.device.execute);
    final status = result['status'];
    _say(switch ((reviewed['change'], status)) {
      ('enable_remote', 'acknowledged') =>
        'Remote access is enabled. This phone verified that the reviewed Box is reachable remotely.',
      ('remove_member', 'applied') => 'The member was removed from the Home.',
      ('revoke_support', 'applied') => 'Support access was revoked.',
      ('grant_support', 'acknowledged') =>
        'Support access was granted. Choose Revoke support to remove it.',
      ('submit_report', 'acknowledged') =>
        'Support accepted the reviewed report.',
      (_, 'uncertain') =>
        'The device outcome is uncertain. Check its current state before making another request.',
      _ => 'Account response · ${status ?? 'unknown'}',
    });
    final next = result['next_step'];
    if (next is Map &&
        next['surface'] == 'hub_device_credentials' &&
        next['hub_ids'] is List) {
      _say([
        next['instructions'],
        for (final id in next['hub_ids']) 'Box · $id',
      ].join('\n'));
    }
  }

  Future<void> _send(String message) async {
    final input = message.trim();
    if (input.isEmpty || !_active || _busy) return;
    _text.clear();
    _say(input, resident: true);
    await _run(() async {
      final lower = input.toLowerCase();
      if (_review != null) {
        if (input == 'Connect reviewed Box') {
          final review = _review!;
          final handoff = await _call('select_hub',
              {'home_id': review['home_id'], 'hub_id': review['hub_id']});
          if (['home_id', 'hub_id', 'server_instance_id']
              .any((k) => handoff[k] != review[k])) {
            throw StateError('Box selection changed');
          }
          await widget.device.select(handoff);
          _say('Connected to the reviewed Box. Confirm when ready.');
        } else if (input == 'Confirm reviewed change') {
          await _confirm();
        } else if (lower == 'cancel') {
          final id = _review!['plan_id'];
          final receipt = await _call('cancel_customer_plan', {'plan_id': id});
          if (receipt['status'] != 'cancelled' || receipt['plan_id'] != id) {
            throw StateError('Cancellation could not be verified');
          }
          _review = null;
          _say('Cancelled on all devices.');
        } else {
          _say('Use Confirm reviewed change or Cancel.');
        }
        return;
      }
      if (lower == 'cancel') {
        _intent = null;
        _items = [];
        _choice = null;
        _say('Cancelled.');
        return;
      }
      if (_intent == 'submit_report' &&
          _home != null &&
          _hub != null &&
          _choice == null) {
        await _plan('submit_report', {
          'home_id': _home,
          'hub_id': _hub,
          'report_summary': input,
          'include_bundle': _includeBundle
        });
        return;
      }
      _intent = null;
      _items = [];
      _choice = null;
      if (lower.contains('pending')) {
        await _list(_Choice.pending);
      } else if (lower.contains('revoke')) {
        await _change('revoke_support');
      } else if (lower.contains('report') ||
          lower.contains('something is wrong') ||
          lower.contains('send this to support')) {
        await _change('submit_report');
      } else if (lower.contains('support')) {
        await _change('grant_support');
      } else if (lower.contains('remote') || lower.contains('outside')) {
        await _change('enable_remote');
      } else if (lower.contains('remove') && lower.contains('member')) {
        await _change('remove_member');
      } else if (lower.contains('member') || lower.contains('who')) {
        _intent = 'members';
        if ((lower.contains('connected') || _home == null) &&
            widget.device.connectedHomeId != null) {
          final connected = widget.device.connectedHomeId;
          final result = await _call('list_homes');
          final matching =
              (result['homes'] as List).where((h) => h['home_id'] == connected);
          if (matching.length == 1) {
            if (_home != connected) {
              _hub = null;
              _instance = null;
            }
            _home = connected;
            _say(
                'This phone is connected to ${matching.single['display_name'] ?? connected}.');
          }
        }
        await _list(_home == null ? _Choice.homes : _Choice.members);
      } else if (lower.contains('join')) {
        await _change('join_home');
      } else if (lower.contains('lighting') && _hub != null) {
        final handoff =
            await _call('select_hub', {'home_id': _home, 'hub_id': _hub});
        if (handoff['server_instance_id'] != _instance ||
            handoff['home_id'] != _home ||
            handoff['hub_id'] != _hub) {
          throw StateError('Identity changed');
        }
        await widget.device.select(handoff);
        if (mounted && _active) await widget.continueOnHub(context, _history);
      } else if (lower.contains('sign in') ||
          lower.contains('password') ||
          lower.contains('delete account') ||
          lower.contains('change plan')) {
        final step = lower.contains('password')
            ? 'password_recovery'
            : lower.contains('delete')
                ? 'account_deletion'
                : lower.contains('plan')
                    ? 'plan_change'
                    : 'sign_in';
        final handoff = await _call('get_account_handoff', {'step': step});
        _say(
            '${handoff['actor'] ?? 'Resident'} · ${handoff['instructions'] ?? handoff['surface'] ?? step.replaceAll('_', ' ')}');
      } else if (lower.contains('status') ||
          lower.contains('backup') ||
          lower.contains('plan')) {
        final status = await _call('read_account_status');
        final backup = status['has_backup'] == true
            ? 'Latest cloud backup · ${status['captured_at'] ?? 'available'}'
            : 'No cloud backup is available.';
        _say('Account plan · ${status['plan'] ?? 'unknown'}\n$backup');
      } else if (lower.contains('home') || lower.contains('hub')) {
        await _list(_Choice.homes);
      } else {
        _say(
            'Choose a Home, account status, support, remote access, or pending actions.');
      }
    });
  }

  @override
  Widget build(BuildContext context) {
    final enabled = _active && !_busy;
    final reporting = _intent == 'submit_report' &&
        _home != null &&
        _hub != null &&
        _choice == null;
    final actions = _review != null
        ? [
            'Confirm reviewed change',
            if ({'grant_support', 'enable_remote'}
                    .contains(_review?['change']) ||
                (_review?['change'] == 'submit_report' &&
                    _review?['include_bundle'] == true))
              'Connect reviewed Box',
            'Cancel'
          ]
        : reporting
            ? ['Cancel']
            : [
                if (_offers('list_homes')) 'Homes',
                if (_offers('list_home_members')) 'Members',
                if (_hub != null) 'Lighting tools',
                if (_offers('read_account_status')) 'Account status',
                if (_offers('plan_customer_change')) ...[
                  'Grant support',
                  'Revoke support',
                  'Enable remote access',
                  'Send report',
                  'Remove member',
                  'Join Home'
                ],
                if (_offers('list_pending_actions')) 'Pending actions',
              ];
    return Scaffold(
        appBar: AppBar(title: const Text('Rhythm assistant')),
        body: SafeArea(
            child: Column(children: [
          Expanded(
              child: ListView(
                  controller: _scroll,
                  padding: const EdgeInsets.all(16),
                  children: [
                if (_active)
                  for (final item in _history)
                    Align(
                        alignment: item.resident
                            ? Alignment.centerRight
                            : Alignment.centerLeft,
                        child: Card(
                            child: Padding(
                                padding: const EdgeInsets.all(12),
                                child: SelectableText(item.text)))),
              ])),
          if (!_active)
            const Text('Account changed. Open a new conversation to continue.'),
          if (_busy) const LinearProgressIndicator(),
          if (_active && _contract == null && !_busy)
            TextButton(
                onPressed: () => _run(_load),
                child: const Text('Retry account connection')),
          if (reporting)
            CheckboxListTile(
                title: const Text('Attach diagnostic bundle'),
                value: _includeBundle,
                onChanged: enabled
                    ? (v) => setState(() => _includeBundle = v ?? false)
                    : null),
          if (_active)
            ConstrainedBox(
                constraints: const BoxConstraints(maxHeight: 190),
                child: SingleChildScrollView(
                    child: Wrap(spacing: 6, children: [
                  for (final row in _items)
                    if (_choice != _Choice.members ||
                        _intent == 'remove_member')
                      ActionChip(
                          label: Text(_label(row)),
                          onPressed:
                              enabled ? () => _run(() => _pick(row)) : null),
                  for (final action in actions)
                    ActionChip(
                        label: Text(action),
                        onPressed: enabled ? () => _send(action) : null),
                ]))),
          Padding(
              padding: const EdgeInsets.all(12),
              child: TextField(
                  controller: _text,
                  enabled: enabled,
                  maxLength: reporting ? 1500 : null,
                  textInputAction: TextInputAction.send,
                  onSubmitted: _send,
                  decoration: InputDecoration(
                      labelText: reporting ? 'Report summary' : 'Message',
                      suffixIcon: IconButton(
                          icon: const Icon(Icons.send),
                          onPressed:
                              enabled ? () => _send(_text.text) : null)))),
        ])));
  }
}
