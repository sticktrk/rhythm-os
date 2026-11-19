import 'package:dio/dio.dart';
import 'package:flutter/material.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

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
      if (mounted && widget.isCurrentTarget()) {
        setState(() {
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
        final registration = await widget.api.registerAssistantClient();
        issued = {
          'Client ID': registration.id,
          'Client secret': registration.secret,
          'One-use activation code (${registration.expiresIn.inMinutes} min)':
              registration.activationCode
        };
      } else {
        final credential = await widget.api.createAgentCredential();
        issued = {'Agent token': credential.token};
      }
      if (mounted && widget.isCurrentTarget()) {
        setState(() {
          _issued = issued;
          _revealed = false;
        });
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
    return Scaffold(
        appBar: AppBar(title: const Text('Assistant access')),
        body: SafeArea(
            child: ListView(padding: const EdgeInsets.all(16), children: [
          if (!active)
            const Text('Connect locally to the same hub to manage access.')
          else ...[
            const Text('MCP endpoint'),
            SelectableText(widget.endpoint),
            if (_error != null) Text(_error!),
            if (_busy) const LinearProgressIndicator(),
            if (!_ready && !_busy)
              TextButton(
                  onPressed: _refresh, child: const Text('Refresh access')),
            if (enabled && _issued.isEmpty) ...[
              const Text(
                  'Create a credential for a client to read this hub, control its lights, and apply confirmed changes.'),
              FilledButton(
                  onPressed: () => _issue(false),
                  child: const Text('Create agent token')),
              OutlinedButton(
                  onPressed: () => _issue(true),
                  child: const Text('Connect Home Assistant Assist')),
            ],
            if (_issued.isNotEmpty) ...[
              Text(_issued.length == 1
                  ? 'Use this bearer token in your MCP client. It is shown once.'
                  : 'In Home Assistant, add MCP Client with this endpoint, client ID and secret. Use the activation code when Rhythm asks for approval.'),
              for (final entry in _issued.entries)
                ListTile(
                    title: Text(entry.key),
                    subtitle: SelectableText(
                        _revealed ? entry.value : '•••• •••• ••••')),
              TextButton(
                  onPressed: () => setState(() => _revealed = !_revealed),
                  child: Text(_revealed ? 'Hide' : 'Reveal')),
              TextButton(
                  onPressed: () => setState(() {
                        _issued = {};
                        _revealed = false;
                      }),
                  child: const Text('Done')),
            ],
            for (final entry in _credentials)
              ListTile(
                  title: Text(entry.kind),
                  subtitle: Text(entry.id),
                  trailing: TextButton(
                      onPressed: enabled ? () => _revoke(entry) : null,
                      child: const Text('Revoke'))),
          ],
        ])));
  }
}
