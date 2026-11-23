import 'dart:convert';

import 'package:dio/dio.dart';
import 'package:flutter/material.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../../widgets/assistant_chat_widgets.dart';
import '../../widgets/settings_row.dart';
import '../../widgets/solar_orbit.dart' show CelestialColors;

/// A separate local-owner surface. Secrets are held only for the one-time
/// display, and never become part of assistant state or a cloud service call.
class AssistantAccessScreen extends StatefulWidget {
  const AssistantAccessScreen(
      {super.key,
      required this.api,
      required this.endpoint,
      required this.isCurrentTarget,
      required this.targetChanges});
  final RhythmAuthApi api;
  final String endpoint;
  final bool Function() isCurrentTarget;
  final Listenable targetChanges;
  @override
  State<AssistantAccessScreen> createState() => _AssistantAccessState();
}

class _AssistantAccessState extends State<AssistantAccessScreen> {
  List<RhythmAgentCredential> _credentials = [];
  List<RhythmAssistantApproval> _approvals = [];
  bool _supportsApprovals = false, _unattended = false;
  Map<String, String> _issued = {};
  bool _busy = true, _ready = false, _revealed = false;
  String? _error;

  @override
  void initState() {
    super.initState();
    widget.targetChanges.addListener(_targetChanged);
    _refresh();
  }

  @override
  void dispose() {
    widget.targetChanges.removeListener(_targetChanged);
    super.dispose();
  }

  void _targetChanged() {
    if (mounted) {
      setState(() {
        if (!widget.isCurrentTarget()) {
          _issued = {};
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
      if (mounted && widget.isCurrentTarget()) {
        setState(() {
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
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _issue(bool homeAssistant) async {
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

  String _reviewLabel(String key) {
    final words = key.replaceAll('_', ' ').trim();
    return words.isEmpty ? words : words[0].toUpperCase() + words.substring(1);
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
                if (_ready && _approvals.isNotEmpty) ...[
                  const SettingsSectionHeader(title: 'Pending approvals'),
                  for (final approval in _approvals)
                    Padding(
                        padding: const EdgeInsets.only(bottom: 10),
                        child: Card(
                            color: CelestialColors.backgroundCard,
                            margin: EdgeInsets.zero,
                            elevation: 0,
                            clipBehavior: Clip.antiAlias,
                            shape: RoundedRectangleBorder(
                                borderRadius: BorderRadius.circular(16),
                                side: const BorderSide(
                                    color: CelestialColors.orbitRing)),
                            // Read the page's theme, not the app's above it.
                            child: Builder(
                                builder: (context) => Theme(
                                data: Theme.of(context).copyWith(
                                    dividerColor: Colors.transparent),
                                child: ExpansionTile(
                                    key: ValueKey(approval.planId),
                                    title: Text(_reviewLabel(approval.operation
                                        .replaceFirst(RegExp(r'^plan_'), ''))),
                                    subtitle: Text(
                                        'Requested by ${approval.credentialId}'),
                                    childrenPadding: const EdgeInsets.fromLTRB(
                                        16, 0, 16, 16),
                                    expandedCrossAxisAlignment:
                                        CrossAxisAlignment.start,
                                    expandedAlignment: Alignment.centerLeft,
                                    children: [
                                      for (final field in approval
                                          .review.entries
                                          .where((entry) => !const {
                                                'plan_id',
                                                'apply_arguments',
                                                'operation',
                                                'correlation_id',
                                                'server_instance_id'
                                              }.contains(entry.key))) ...[
                                        AssistantNote(_reviewLabel(field.key)),
                                        SelectableText(field.value is String
                                            ? field.value as String
                                            : const JsonEncoder.withIndent('  ')
                                                .convert(field.value)),
                                        gap,
                                      ],
                                      Wrap(spacing: 8, children: [
                                        FilledButton(
                                            onPressed: enabled
                                                ? () => _decide(approval, true)
                                                : null,
                                            child: const Text('Approve')),
                                        TextButton(
                                            onPressed: enabled
                                                ? () => _decide(approval, false)
                                                : null,
                                            child: const Text('Decline')),
                                      ]),
                                    ]))))),
                ],
                if (enabled && _issued.isEmpty) ...[
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
                          label: entry.kind,
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
