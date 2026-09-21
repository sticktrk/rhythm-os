import 'dart:convert';
import 'package:flutter/material.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:uuid/uuid.dart';
import '../../services/analytics_service.dart';
import '../../services/cloud_backed_server_api.dart';

class SavedWifiScreen extends StatefulWidget {
  const SavedWifiScreen({super.key, required this.api, this.choose = false});
  final CloudBackedServerApi api;
  final bool choose;
  static Future<String?> select(
    BuildContext context,
    CloudBackedServerApi api,
  ) => Navigator.of(context).push<String>(
    MaterialPageRoute(builder: (_) => SavedWifiScreen(api: api, choose: true)),
  );
  @override
  State<SavedWifiScreen> createState() => _SavedWifiScreenState();
}

class _SavedWifiScreenState extends State<SavedWifiScreen> {
  RhythmWifiProfiles? _catalog;
  String? _selection;
  String? _error;
  bool _busy = true;
  final _journey = const Uuid().v4();
  bool _usedEntryJourney = false;
  @override
  void initState() {
    super.initState();
    AnalyticsService().logWifiAction(
      networkChange: false,
      journeyId: _journey,
      action: 'entry',
      outcome: 'opened',
    );
    _load();
  }

  Future<void> _load() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final catalog = await widget.api.getWifiProfiles();
      if (!mounted) return;
      setState(() {
        _catalog = catalog;
        _selection = catalog.profiles.any((p) => p.id == _selection)
            ? _selection
            : catalog.defaultId;
      });
    } catch (_) {
      if (mounted) {
        setState(
          () => _error =
              'Could not load saved networks. Check the Box connection and owner access.',
        );
      }
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<void> _write(
    String action, {
    String? id,
    String? ssid,
    String? password,
  }) async {
    final catalog = _catalog;
    if (_busy || catalog == null) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    final journey = _usedEntryJourney ? const Uuid().v4() : _journey;
    if (_usedEntryJourney) {
      AnalyticsService().logWifiAction(
        networkChange: false,
        journeyId: journey,
        action: 'entry',
        outcome: 'opened',
      );
    }
    _usedEntryJourney = true;
    AnalyticsService().logWifiAction(
      networkChange: false,
      journeyId: journey,
      action: action,
      outcome: 'attempt',
    );
    try {
      final updated = await widget.api.updateWifiProfile(
        revision: catalog.revision,
        action: action,
        correlationId: journey,
        id: id,
        ssid: ssid,
        password: password,
      );
      if (!mounted) return;
      setState(() {
        _catalog = updated;
        if (action == 'save' && id == null) {
          _selection = updated.profiles.last.id;
        }
        if (!updated.profiles.any((p) => p.id == _selection)) {
          _selection = updated.defaultId;
        }
      });
      AnalyticsService().logWifiAction(
        networkChange: false,
        journeyId: journey,
        action: action,
        outcome: 'succeeded',
      );
    } catch (_) {
      if (mounted) {
        setState(
          () => _error =
              'Could not confirm the edit. Reload saved networks before trying again.',
        );
      }
      AnalyticsService().logWifiAction(
        networkChange: false,
        journeyId: journey,
        action: action,
        outcome: 'failed',
      );
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  String? _roles(RhythmWifiProfile profile) {
    final roles = [
      if (_catalog?.defaultId == profile.id) 'Default for new accessories',
      if (_catalog?.boxProfileId == profile.id) 'Rhythm Box connection',
    ];
    return roles.isEmpty ? null : roles.join(' · ');
  }

  Future<void> _edit([RhythmWifiProfile? profile]) async {
    final result = await showDialog<({String ssid, String? password})>(
      context: context,
      builder: (_) => _WifiEditor(profile: profile),
    );
    if (result != null && mounted) {
      await _write(
        'save',
        id: profile?.id,
        ssid: result.ssid,
        password: result.password,
      );
    }
  }

  @override
  Widget build(BuildContext context) => Scaffold(
    appBar: AppBar(
      title: Text(
        widget.choose ? 'Choose network' : 'Wi-Fi for new accessories',
      ),
    ),
    body: ListView(
      padding: const EdgeInsets.all(20),
      children: [
        const Text(
          'New accessories join the default network during setup. Changing the default does not move your Rhythm Box or accessories you already added.',
        ),
        const SizedBox(height: 16),
        if (_busy) const LinearProgressIndicator(),
        if (_error != null) ...[
          Text(_error!, key: const ValueKey('wifi-error')),
          TextButton(
            onPressed: _busy ? null : _load,
            child: const Text('Reload'),
          ),
        ],
        if (_catalog?.profiles.isEmpty == true)
          const Padding(
            padding: EdgeInsets.symmetric(vertical: 20),
            child: Text('No saved networks. Add one to continue.'),
          ),
        for (final profile in _catalog?.profiles ?? <RhythmWifiProfile>[])
          ListTile(
            key: ValueKey('wifi-profile-${profile.id}'),
            // Settings: the radio is the default for new accessories.
            // Chooser: the radio is this one attempt's target.
            leading: Icon(
              (widget.choose
                      ? _selection == profile.id
                      : _catalog!.defaultId == profile.id)
                  ? Icons.radio_button_checked
                  : Icons.radio_button_off,
            ),
            title: Text(profile.ssid),
            subtitle: _roles(profile) == null ? null : Text(_roles(profile)!),
            onTap: _busy
                ? null
                : () {
                    if (widget.choose) {
                      setState(() => _selection = profile.id);
                    } else if (_catalog!.defaultId != profile.id) {
                      _write('default', id: profile.id);
                    }
                  },
            // The Box connection is changed by moving the Box, never here.
            trailing: _catalog!.boxProfileId == profile.id
                ? null
                : PopupMenuButton<String>(
                    key: ValueKey('wifi-profile-menu-${profile.id}'),
                    enabled: !_busy,
                    onSelected: (action) {
                      if (action == 'edit') {
                        _edit(profile);
                      } else {
                        _write(action, id: profile.id);
                      }
                    },
                    itemBuilder: (_) => [
                      const PopupMenuItem(value: 'edit', child: Text('Edit')),
                      // Removing the default would pick the network
                      // for new accessories on the owner's behalf.
                      if (_catalog!.defaultId != profile.id ||
                          _catalog!.profiles.length == 1)
                        const PopupMenuItem(
                          value: 'remove',
                          child: Text('Remove saved network'),
                        ),
                    ],
                  ),
          ),
        OutlinedButton.icon(
          key: const ValueKey('wifi-add'),
          onPressed: _busy ? null : _edit,
          icon: const Icon(Icons.add),
          label: const Text('Add network'),
        ),
        if (widget.choose)
          FilledButton(
            key: const ValueKey('wifi-use'),
            onPressed: _busy || _selection == null || _error != null
                ? null
                : () {
                    AnalyticsService().logWifiAction(
                      networkChange: false,
                      journeyId: _journey,
                      action: 'select',
                      outcome: 'succeeded',
                    );
                    Navigator.pop(context, _selection);
                  },
            child: const Text('Use selected network'),
          ),
        const SizedBox(height: 16),
        const Text(
          'Passwords stay on this Box and are excluded from backups. Removing a saved network does not disconnect devices already using it.',
        ),
      ],
    ),
  );
}

class _WifiEditor extends StatefulWidget {
  const _WifiEditor({this.profile});
  final RhythmWifiProfile? profile;
  @override
  State<_WifiEditor> createState() => _WifiEditorState();
}

class _WifiEditorState extends State<_WifiEditor> {
  late final _ssid = TextEditingController(text: widget.profile?.ssid);
  final _password = TextEditingController();
  bool _open = false;
  String? _error;
  @override
  void dispose() {
    _ssid.dispose();
    _password.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
    title: Text(widget.profile == null ? 'Add network' : 'Edit network'),
    content: SingleChildScrollView(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          TextField(
            controller: _ssid,
            decoration: const InputDecoration(labelText: 'Network name'),
            autocorrect: false,
          ),
          TextField(
            controller: _password,
            obscureText: true,
            autocorrect: false,
            enableSuggestions: false,
            decoration: InputDecoration(
              labelText: 'Password',
              helperText: widget.profile != null
                  ? 'Leave blank to keep the saved password'
                  : null,
            ),
          ),
          SwitchListTile(
            contentPadding: EdgeInsets.zero,
            title: const Text('Open network'),
            value: _open,
            onChanged: (v) => setState(() => _open = v),
          ),
          if (_error != null) Text(_error!),
        ],
      ),
    ),
    actions: [
      TextButton(
        onPressed: () => Navigator.pop(context),
        child: const Text('Cancel'),
      ),
      FilledButton(
        onPressed: () {
          final ssidBytes = utf8.encode(_ssid.text).length;
          final passwordBytes = utf8.encode(_password.text).length;
          final keepsPassword =
              widget.profile != null && _password.text.isEmpty;
          final validPassword =
              _open ||
              keepsPassword ||
              (passwordBytes >= 8 && passwordBytes <= 63) ||
              RegExp(r'^[0-9a-fA-F]{64}$').hasMatch(_password.text);
          if (ssidBytes < 1 ||
              ssidBytes > 32 ||
              _ssid.text.contains('\u0000') ||
              _password.text.contains('\u0000') ||
              !validPassword) {
            setState(
              () => _error =
                  'Use a network name of 1–32 bytes and an 8–63 byte password (or 64 hex digits), or select Open network.',
            );
            return;
          }
          Navigator.pop(context, (
            ssid: _ssid.text,
            password: _open
                ? ''
                : _password.text.isEmpty && widget.profile != null
                ? null
                : _password.text,
          ));
        },
        child: const Text('Save'),
      ),
    ],
  );
}
