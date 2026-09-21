import 'dart:convert';
import 'package:flutter/material.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:uuid/uuid.dart';
import '../../services/analytics_service.dart';
import '../../services/cloud_backed_server_api.dart';
import '../../widgets/solar_orbit.dart';
import 'network_ui.dart';

class SavedWifiScreen extends StatefulWidget {
  const SavedWifiScreen({super.key, required this.api, this.choose = false});
  final CloudBackedServerApi api;
  final bool choose;
  static Future<String?> select(
    BuildContext context,
    CloudBackedServerApi api,
  ) =>
      Navigator.of(context).push<String>(
        MaterialPageRoute(
            builder: (_) => SavedWifiScreen(api: api, choose: true)),
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

  Widget _trailing(RhythmWifiProfile profile) {
    final catalog = _catalog!;
    final selected = widget.choose
        ? _selection == profile.id
        : catalog.defaultId == profile.id;
    final removable =
        catalog.defaultId != profile.id || catalog.profiles.length == 1;
    return Row(mainAxisSize: MainAxisSize.min, children: [
      Icon(
        selected
            ? Icons.radio_button_checked_rounded
            : Icons.radio_button_off_rounded,
        color: selected
            ? networkTeal
            : CelestialColors.textSecondary.withValues(alpha: 0.5),
        size: 22,
      ),
      // The Box connection is changed by moving the Box, never here. Keep the
      // column width so every radio aligns.
      if (catalog.boxProfileId == profile.id)
        const SizedBox(width: 48)
      else
        PopupMenuButton<String>(
            key: ValueKey('wifi-profile-menu-${profile.id}'),
            enabled: !_busy,
            tooltip: 'Network options',
            color: CelestialColors.backgroundCard,
            icon: Icon(Icons.more_vert_rounded,
                color: CelestialColors.textSecondary.withValues(alpha: 0.8),
                size: 20),
            onSelected: (action) {
              if (action == 'edit') {
                _edit(profile);
              } else {
                _write(action, id: profile.id);
              }
            },
            itemBuilder: (_) => [
                  const PopupMenuItem(
                      value: 'edit',
                      child: Text('Edit',
                          style:
                              TextStyle(color: CelestialColors.textPrimary))),
                  // Removing the default would pick the network for new
                  // accessories on the owner's behalf.
                  if (removable)
                    const PopupMenuItem(
                        value: 'remove',
                        child: Text('Remove saved network',
                            style: TextStyle(color: Color(0xFFEF4444))))
                ]),
    ]);
  }

  @override
  Widget build(BuildContext context) {
    final profiles = _catalog?.profiles ?? const <RhythmWifiProfile>[];
    return NetworkScaffold(
        title: widget.choose ? 'Choose network' : 'Wi-Fi for new accessories',
        busy: _busy,
        footer: widget.choose
            ? FilledButton(
                key: const ValueKey('wifi-use'),
                style: networkPrimaryButtonStyle,
                onPressed: _busy || _selection == null || _error != null
                    ? null
                    : () {
                        AnalyticsService().logWifiAction(
                            networkChange: false,
                            journeyId: _journey,
                            action: 'select',
                            outcome: 'succeeded');
                        Navigator.pop(context, _selection);
                      },
                child: const Text('Use selected network'))
            : null,
        children: [
          NetworkBodyText(widget.choose
              ? 'Choose the network this bulb should move to. Your default for new accessories does not change.'
              : 'New accessories join the default network during setup. Changing the default does not move your Rhythm Box or accessories you already added.'),
          if (_error != null)
            NetworkNotice(
                key: const ValueKey('wifi-error'),
                text: _error!,
                action: TextButton(
                    onPressed: _busy ? null : _load,
                    style: TextButton.styleFrom(foregroundColor: networkTeal),
                    child: const Text('Reload'))),
          const NetworkSectionHeader('SAVED NETWORKS'),
          if (_catalog != null && profiles.isEmpty)
            const Padding(
                padding: EdgeInsets.fromLTRB(4, 4, 4, 18),
                child:
                    NetworkBodyText('No saved networks. Add one to continue.')),
          for (final profile in profiles)
            NetworkCard(
                key: ValueKey('wifi-profile-${profile.id}'),
                icon: _catalog!.boxProfileId == profile.id
                    ? Icons.router_rounded
                    : Icons.wifi_rounded,
                title: profile.ssid,
                subtitle: _roles(profile),
                highlighted: widget.choose
                    ? _selection == profile.id
                    : _catalog!.defaultId == profile.id,
                // Settings: tapping chooses the default for new accessories.
                // Chooser: tapping picks this one attempt's target.
                onTap: _busy
                    ? null
                    : () {
                        if (widget.choose) {
                          setState(() => _selection = profile.id);
                        } else if (_catalog!.defaultId != profile.id) {
                          _write('default', id: profile.id);
                        }
                      },
                trailing: _trailing(profile)),
          const SizedBox(height: 4),
          OutlinedButton.icon(
              key: const ValueKey('wifi-add'),
              style: networkSecondaryButtonStyle,
              onPressed: _busy || _catalog == null ? null : _edit,
              icon: const Icon(Icons.add_rounded, size: 18),
              label: const Text('Add network')),
          const SizedBox(height: 20),
          const Padding(
              padding: EdgeInsets.symmetric(horizontal: 4),
              child: NetworkBodyText(
                  'Passwords stay on this Box and are excluded from backups. Removing a saved network does not disconnect devices already using it.')),
        ]);
  }
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
  bool _reveal = false;
  String? _error;
  @override
  void dispose() {
    _ssid.dispose();
    _password.dispose();
    super.dispose();
  }

  InputDecoration _field(String label, IconData icon, {String? helper}) =>
      InputDecoration(
          labelText: label,
          helperText: helper,
          helperMaxLines: 2,
          prefixIcon: Icon(icon),
          suffixIcon: icon == Icons.lock_outline_rounded
              ? IconButton(
                  tooltip: _reveal ? 'Hide password' : 'Show password',
                  icon: Icon(_reveal
                      ? Icons.visibility_off_outlined
                      : Icons.visibility_outlined),
                  onPressed: () => setState(() => _reveal = !_reveal))
              : null);

  @override
  Widget build(BuildContext context) => AlertDialog(
        backgroundColor: CelestialColors.backgroundCard,
        title: Text(widget.profile == null ? 'Add network' : 'Edit network',
            style: const TextStyle(color: CelestialColors.textPrimary)),
        content: SingleChildScrollView(
            child: Column(mainAxisSize: MainAxisSize.min, children: [
          TextField(
              controller: _ssid,
              autofocus: widget.profile == null,
              autocorrect: false,
              enableSuggestions: false,
              textInputAction: TextInputAction.next,
              style: const TextStyle(color: CelestialColors.textPrimary),
              decoration: _field('Network name', Icons.wifi_rounded)),
          const SizedBox(height: 12),
          TextField(
              controller: _password,
              enabled: !_open,
              obscureText: !_reveal,
              autocorrect: false,
              enableSuggestions: false,
              style: const TextStyle(color: CelestialColors.textPrimary),
              decoration: _field('Password', Icons.lock_outline_rounded,
                  helper: widget.profile != null && !_open
                      ? 'Leave blank to keep the saved password'
                      : null)),
          const SizedBox(height: 4),
          SwitchListTile(
              contentPadding: EdgeInsets.zero,
              activeThumbColor: networkTeal,
              title: const Text('Open network',
                  style: TextStyle(
                      color: CelestialColors.textPrimary, fontSize: 15)),
              subtitle: const Text('No password',
                  style: TextStyle(
                      color: CelestialColors.textSecondary, fontSize: 13)),
              value: _open,
              onChanged: (v) => setState(() => _open = v)),
          if (_error != null)
            Padding(
                padding: const EdgeInsets.only(top: 8),
                child:
                    NetworkBodyText(_error!, color: CelestialColors.warning)),
        ])),
        actions: [
          TextButton(
              onPressed: () => Navigator.pop(context),
              child: const Text('Cancel',
                  style: TextStyle(color: CelestialColors.textSecondary))),
          TextButton(
            style: TextButton.styleFrom(foregroundColor: networkTeal),
            onPressed: () {
              final ssidBytes = utf8.encode(_ssid.text).length;
              final passwordBytes = utf8.encode(_password.text).length;
              final keepsPassword =
                  widget.profile != null && _password.text.isEmpty;
              final validPassword = _open ||
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
                      'Enter a network name (up to 32 characters) and a password of 8–63 characters, or turn on Open network.',
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
