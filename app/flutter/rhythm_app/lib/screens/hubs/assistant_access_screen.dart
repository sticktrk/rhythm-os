import 'dart:async';

import 'package:dio/dio.dart';
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../../services/assistant_attention_controller.dart';
import '../../services/push_registration_service.dart';
import '../../widgets/assistant_attention_sheet.dart';
import '../../widgets/assistant_chat_widgets.dart';
import '../../widgets/settings_row.dart';
import '../../widgets/assistant_plan_review.dart';
import '../../widgets/solar_orbit.dart' show CelestialColors;

/// A separate local-owner surface. Secrets are held only for the one-time
/// display, and never become part of assistant state or a cloud service call.
class AssistantAccessScreen extends StatefulWidget {
  const AssistantAccessScreen(
      {super.key,
      required this.api,
      required this.endpoint,
      required this.isCurrentTarget,
      required this.targetChanges,
      this.attentionEvents});
  final RhythmAuthApi api;
  final String endpoint;
  final bool Function() isCurrentTarget;
  final Listenable targetChanges;
  final Stream<dynamic>? attentionEvents;
  @override
  State<AssistantAccessScreen> createState() => _AssistantAccessState();
}

class _AssistantAccessState extends State<AssistantAccessScreen> {
  List<RhythmAgentCredential> _credentials = [];
  List<RhythmAssistantApproval> _approvals = [];
  List<RhythmAssistantConnection> _connections = [];
  bool _supportsConnections = false,
      _instructions = false,
      _refreshAgain = false;
  StreamSubscription<dynamic>? _attention;
  bool _supportsApprovals = false, _unattended = false;
  Map<String, String> _issued = {};
  bool _busy = true, _ready = false, _revealed = false;
  String? _error;

  @override
  void initState() {
    super.initState();
    widget.targetChanges.addListener(_targetChanged);
    _attention = widget.attentionEvents?.listen((_) {
      if (_busy) {
        _refreshAgain = true;
      } else {
        unawaited(_refresh());
      }
    });
    unawaited(PushRegistrationService.instance.requestForAssistant());
    _refresh();
  }

  @override
  void dispose() {
    _attention?.cancel();
    widget.targetChanges.removeListener(_targetChanged);
    super.dispose();
  }

  void _targetChanged() {
    if (mounted) {
      setState(() {
        if (!widget.isCurrentTarget()) {
          _issued = {};
          _instructions = false;
          _connections = [];
          _revealed = false;
        }
      });
    }
  }

  Future<void> _refresh([Future<void> Function()? action]) async {
    if (!widget.isCurrentTarget()) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      if (action != null) await action();
      if (!mounted || !widget.isCurrentTarget()) return;
      final entries = await widget.api.listAgentCredentials();
      List<RhythmAssistantApproval> approvals;
      bool supportsApprovals = true;
      try {
        approvals = await widget.api.listAssistantApprovals();
      } on DioException catch (error) {
        if (error.response?.statusCode != 404) rethrow;
        // Older hubs still support listing and revoking registered clients.
        approvals = [];
        supportsApprovals = false;
      }
      List<RhythmAssistantConnection> connections;
      bool supportsConnections = true;
      try {
        if (!mounted || !widget.isCurrentTarget()) return;
        connections = await widget.api.listAssistantConnections();
      } on DioException catch (error) {
        if (error.response?.statusCode != 404) rethrow;
        supportsConnections = false;
        connections = [];
      }
      if (mounted && widget.isCurrentTarget()) {
        setState(() {
          _supportsConnections = supportsConnections;
          _connections = connections;
          _approvals = approvals;
          _supportsApprovals = supportsApprovals;
          _credentials = entries;
          _ready = true;
        });
      }
    } catch (error) {
      if (mounted) {
        setState(() {
          _ready = false;
          final code =
              error is DioException ? error.response?.statusCode : null;
          _error = switch (code) {
            404 => 'This hub does not advertise assistant access management.',
            401 ||
            403 =>
              'Connect locally as the hub owner to manage assistant access.',
            _ =>
              'Could not verify the result. Refresh the access list before trying again.',
          };
        });
      }
    } finally {
      if (mounted) {
        setState(() => _busy = false);
        if (_refreshAgain) {
          _refreshAgain = false;
          unawaited(_refresh());
        }
      }
    }
  }

  Future<void> _reviewConnection(RhythmAssistantConnection connection) async {
    if (!widget.isCurrentTarget()) return;
    final controller = context.read<AssistantAttentionController?>();
    if (controller != null && controller.isCurrent) {
      controller.reviewAgain(connection.id);
    } else {
      await showAssistantAttentionSheet(context,
          request: ConnectionAttention(connection),
          api: widget.api,
          isCurrentTarget: widget.isCurrentTarget,
          changes: widget.targetChanges);
      if (mounted) await _refresh();
    }
  }

  Future<void> _issue(bool homeAssistant) async {
    if (homeAssistant && _supportsConnections) {
      setState(() => _instructions = true);
      return;
    }
    await _refresh(() async {
      Map<String, String> issued;
      if (homeAssistant) {
        final registration = await widget.api
            .registerAssistantClient(unattendedChanges: _unattended);
        issued = {
          'Client ID': registration.id,
          'Client secret': registration.secret,
          'One-use activation code (${registration.expiresIn.inMinutes} min)':
              registration.activationCode
        };
      } else {
        final credential = await widget.api
            .createAgentCredential(unattendedChanges: _unattended);
        issued = {'Agent token': credential.token};
      }
      if (mounted && widget.isCurrentTarget()) {
        setState(() {
          _unattended = false;
          _issued = issued;
          _revealed = false;
        });
      }
    });
  }

  Future<void> _decide(RhythmAssistantApproval approval, bool approve) async {
    await _refresh(() async {
      final receipt = await widget.api
          .decideAssistantApproval(approval.planId, approve: approve);
      if (mounted && widget.isCurrentTarget()) {
        final status = receipt['status'] as String?;
        ScaffoldMessenger.of(context).showSnackBar(SnackBar(
            content: Text(status == null
                ? 'The hub returned its decision receipt.'
                : status.replaceAll('_', ' '))));
      }
    });
  }

  Future<void> _revoke(RhythmAgentCredential entry) async {
    final yes = await showDialog<bool>(
        context: context,
        builder: (dialog) => AlertDialog(
                title: const Text('Revoke assistant access?'),
                content: Text('${entry.kind} · ${entry.id}'),
                actions: [
                  TextButton(
                      onPressed: () => Navigator.pop(dialog, false),
                      child: const Text('Cancel')),
                  FilledButton(
                      onPressed: () => Navigator.pop(dialog, true),
                      child: const Text('Revoke')),
                ]));
    if (yes == true && mounted && widget.isCurrentTarget()) {
      setState(() {
        _issued = {};
        _revealed = false;
      });
      await _refresh(() => widget.api.revokeAgentCredential(entry.id));
    }
  }

  @override
  Widget build(BuildContext context) {
    final active = widget.isCurrentTarget();
    final enabled = active && _ready && !_busy;
    const gap = SizedBox(height: 12);
    return RhythmAssistantPage(
        title: 'Assistant access',
        body: ListView(
            padding: const EdgeInsets.fromLTRB(20, 8, 20, 24),
            children: [
              if (!active)
                const Text('Connect locally to the same hub to manage access.')
              else ...[
                if (!_instructions)
                  AssistantReviewCard(children: [
                    const AssistantNote('MCP endpoint'),
                    SelectableText(widget.endpoint),
                  ]),
                gap,
                if (_error != null) ...[Text(_error!), gap],
                if (_busy) const AssistantWorking(label: 'Checking access…'),
                if (!_busy)
                  Align(
                      alignment: Alignment.centerLeft,
                      child: TextButton(
                          onPressed: _refresh,
                          child: const Text('Refresh access'))),
                if (_ready &&
                    _supportsConnections &&
                    _connections.isNotEmpty) ...[
                  const SettingsSectionHeader(title: 'Waiting to connect'),
                  SettingsGroup(children: [
                    for (final connection in _connections)
                      SettingsRow(
                          icon: Icons.link,
                          label: connection.destination,
                          subtitle: connection.remote
                              ? 'Remote connection request'
                              : null,
                          onTap: enabled
                              ? () => _reviewConnection(connection)
                              : null),
                  ]),
                  gap,
                ],
                if (_ready && _approvals.isNotEmpty) ...[
                  const SettingsSectionHeader(title: 'Pending approvals'),
                  for (final approval in _approvals)
                    Padding(
                        padding: const EdgeInsets.only(bottom: 10),
                        child: AssistantPlanReview(
                            approval: approval,
                            enabled: enabled,
                            onDecide: (approve) => _decide(approval, approve))),
                ],
                if (enabled && _issued.isEmpty && !_instructions) ...[
                  gap,
                  const Text(
                      'Create a credential for a client to read this hub and control its lights.'),
                  gap,
                  if (_supportsApprovals) ...[
                    SettingsGroup(children: [
                      SettingsRow(
                          icon: Icons.auto_mode_outlined,
                          iconColor: const Color(0xFF607D8B),
                          label: 'Allow unattended changes',
                          subtitle:
                              'Lifecycle and ownership changes always require your approval.',
                          showChevron: false,
                          trailing: SettingsSwitch(
                              value: _unattended,
                              onChanged: (value) =>
                                  setState(() => _unattended = value)),
                          onTap: () =>
                              setState(() => _unattended = !_unattended)),
                    ]),
                    gap,
                  ],
                  FilledButton(
                      onPressed: () => _issue(false),
                      child: const Text('Create agent token')),
                  const SizedBox(height: 8),
                  OutlinedButton(
                      onPressed: () => _issue(true),
                      child: const Text('Connect Home Assistant Assist')),
                ],
                if (_instructions && _supportsConnections) ...[
                  gap,
                  AssistantReviewCard(children: [
                    const AssistantNote('Connect Home Assistant Assist'),
                    SelectableText(widget.endpoint),
                    gap,
                    const Text(
                        'In Home Assistant, type rhythm for both the client ID and the client secret. You will approve the connection here.'),
                    gap,
                    FilledButton(
                        onPressed: () => setState(() => _instructions = false),
                        child: const Text('Done')),
                  ]),
                ],
                if (_issued.isNotEmpty) ...[
                  gap,
                  AssistantReviewCard(children: [
                    Text(_issued.length == 1
                        ? 'Use this bearer token in your MCP client. It is shown once.'
                        : 'In Home Assistant, add MCP Client with this endpoint, client ID and secret. Use the activation code when Rhythm asks for approval.'),
                    for (final entry in _issued.entries) ...[
                      gap,
                      AssistantNote(entry.key),
                      SelectableText(
                          _revealed ? entry.value : '•••• •••• ••••'),
                    ],
                    gap,
                    Wrap(spacing: 8, children: [
                      FilledButton(
                          onPressed: () => setState(() {
                                _issued = {};
                                _revealed = false;
                              }),
                          child: const Text('Done')),
                      TextButton(
                          onPressed: () =>
                              setState(() => _revealed = !_revealed),
                          child: Text(_revealed ? 'Hide' : 'Reveal')),
                    ]),
                  ]),
                ],
                if (_credentials.isNotEmpty) ...[
                  const SettingsSectionHeader(title: 'Credentials'),
                  SettingsGroup(children: [
                    for (final entry in _credentials)
                      SettingsRow(
                          icon: Icons.key,
                          iconColor: CelestialColors.accentBlue,
                          label: entry.destination ?? entry.kind,
                          subtitle: entry.unattendedChanges
                              ? '${entry.id} · unattended changes allowed'
                              : entry.id,
                          showChevron: false,
                          trailing: TextButton(
                              onPressed: enabled ? () => _revoke(entry) : null,
                              child: const Text('Revoke'))),
                  ]),
                ],
              ],
            ]));
  }
}
