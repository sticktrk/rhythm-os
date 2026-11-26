import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../services/assistant_attention_controller.dart';
import 'assistant_chat_widgets.dart';
import 'assistant_plan_review.dart';
import 'settings_row.dart';
import 'solar_orbit.dart';

Future<void> showAssistantAttentionSheet(
  BuildContext context, {
  required AssistantAttentionRequest request,
  required RhythmAuthApi api,
  required bool Function() isCurrentTarget,
  required Listenable changes,
}) async {
  if (!isCurrentTarget()) return;
  final messenger = ScaffoldMessenger.maybeOf(context);
  final message = await showModalBottomSheet<String>(
      context: context,
      useRootNavigator: true,
      useSafeArea: true,
      isScrollControlled: true,
      showDragHandle: true,
      backgroundColor: CelestialColors.backgroundDark,
      builder: (_) => AssistantAttentionSheet(
          request: request,
          api: api,
          isCurrentTarget: isCurrentTarget,
          changes: changes));
  if (message != null && context.mounted && messenger?.mounted == true) {
    messenger!.showSnackBar(SnackBar(content: Text(message)));
  }
}

class AssistantAttentionSheet extends StatefulWidget {
  const AssistantAttentionSheet(
      {super.key,
      required this.request,
      required this.api,
      required this.isCurrentTarget,
      required this.changes});
  final AssistantAttentionRequest request;
  final RhythmAuthApi api;
  final bool Function() isCurrentTarget;
  final Listenable changes;

  @override
  State<AssistantAttentionSheet> createState() =>
      _AssistantAttentionSheetState();
}

class _AssistantAttentionSheetState extends State<AssistantAttentionSheet> {
  final _code = TextEditingController();
  bool _busy = false, _unattended = false;
  String? _error;
  Timer? _expiry;

  @override
  void initState() {
    super.initState();
    widget.changes.addListener(_checkTarget);
    final expiry = widget.request.expiresAt;
    if (expiry != null) {
      _expiry = Timer(expiry.difference(DateTime.now()),
          () => _close('This request expired.'));
    }
    WidgetsBinding.instance.addPostFrameCallback((_) => _checkTarget());
  }

  @override
  void dispose() {
    widget.changes.removeListener(_checkTarget);
    _expiry?.cancel();
    _code.dispose();
    super.dispose();
  }

  void _checkTarget() {
    if (mounted && !widget.isCurrentTarget()) {
      // Provider changes can arrive during build. The guard also runs at send.
      WidgetsBinding.instance.addPostFrameCallback((_) => _close(null));
      WidgetsBinding.instance.ensureVisualUpdate();
    }
  }

  void _close(String? message) {
    if (!mounted) return;
    final route = ModalRoute.of<String>(context);
    if (route?.isActive != true) return;
    if (route!.isCurrent) {
      route.navigator!.pop(message);
    } else {
      route.navigator!.removeRoute(route, message);
    }
  }

  Future<void> _decide(bool approve) async {
    if (_busy || !widget.isCurrentTarget()) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      switch (widget.request) {
        case ConnectionAttention(:final connection):
          final result = await widget.api.decideAssistantConnection(
              connection.id,
              approve: approve,
              matchCode: _code.text,
              unattendedChanges: _unattended);
          if (!mounted || !widget.isCurrentTarget()) return;
          switch (result.status) {
            case RhythmConnectionDecision.codeMismatch:
              final attempts = result.attemptsLeft!;
              if (attempts == 0) {
                _close('The code did not match. The request was declined.');
              } else {
                setState(() {
                  _code.clear();
                  _error =
                      'Code did not match. $attempts ${attempts == 1 ? 'attempt' : 'attempts'} left.';
                });
              }
            case RhythmConnectionDecision.expired:
              _close('This request expired.');
            case RhythmConnectionDecision.approved:
              _close('Connection approved.');
            case RhythmConnectionDecision.declined:
              _close('Connection declined.');
          }
        case PlanAttention(:final plan):
          final receipt = await widget.api
              .decideAssistantApproval(plan.planId, approve: approve);
          if (!mounted || !widget.isCurrentTarget()) return;
          final status = receipt['status'] as String?;
          _close(status?.replaceAll('_', ' ') ??
              'The hub returned its decision receipt.');
      }
    } catch (_) {
      if (mounted && widget.isCurrentTarget()) {
        setState(() => _error =
            'Could not verify the decision. Check the request in Assistant access.');
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    if (!widget.isCurrentTarget()) return const SizedBox.shrink();
    final request = widget.request;
    return SingleChildScrollView(
      padding: EdgeInsets.fromLTRB(
          20, 0, 20, 24 + MediaQuery.viewInsetsOf(context).bottom),
      child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (request is ConnectionAttention)
              AssistantReviewCard(children: [
                Text(
                    '${request.connection.destination} wants to connect to your Rhythm hub',
                    style: Theme.of(context).textTheme.titleLarge),
                if (request.connection.remote)
                  const Text(
                      'This connection was requested through remote access.'),
                const SizedBox(height: 16),
                const Text(
                    'Enter the code shown on the screen you are setting up'),
                TextField(
                    controller: _code,
                    keyboardType: TextInputType.number,
                    maxLength: 2,
                    inputFormatters: [FilteringTextInputFormatter.digitsOnly],
                    onChanged: (_) => setState(() {}),
                    enabled: !_busy,
                    decoration:
                        const InputDecoration(labelText: 'Two-digit code')),
                ExpansionTile(
                    title: const Text('Connection options'),
                    tilePadding: EdgeInsets.zero,
                    children: [
                      SettingsRow(
                          icon: Icons.auto_mode_outlined,
                          label: 'Allow unattended changes',
                          subtitle:
                              'Lifecycle and ownership changes always require your approval.',
                          showChevron: false,
                          trailing: SettingsSwitch(
                              value: _unattended,
                              onChanged: _busy
                                  ? null
                                  : (v) => setState(() => _unattended = v))),
                    ]),
                const SizedBox(height: 12),
                Wrap(spacing: 8, children: [
                  FilledButton(
                      onPressed: !_busy && _code.text.length == 2
                          ? () => _decide(true)
                          : null,
                      child: const Text('Approve')),
                  TextButton(
                      onPressed: _busy ? null : () => _decide(false),
                      child: const Text('Decline')),
                ]),
              ])
            else if (request is PlanAttention)
              AssistantPlanReview(
                  approval: request.plan,
                  initiallyExpanded: true,
                  enabled: !_busy,
                  onDecide: _decide),
            if (_error != null)
              Padding(
                  padding: const EdgeInsets.only(top: 12),
                  child: Semantics(liveRegion: true, child: Text(_error!))),
          ]),
    );
  }
}
