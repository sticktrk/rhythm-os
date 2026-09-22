import 'dart:convert';
import 'package:flutter/material.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:uuid/uuid.dart';
import '../../services/analytics_service.dart';
import '../../services/cloud_backed_server_api.dart';
import '../../widgets/solar_orbit.dart';
import 'network_ui.dart';

class SavedWifiScreen extends StatefulWidget {
  const SavedWifiScreen({
    super.key,
    required this.api,
    this.choose = false,
    @visibleForTesting this.checkPollInterval = const Duration(seconds: 3),
  });
  final CloudBackedServerApi api;
  final bool choose;
  final Duration checkPollInterval;
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
  String? _checking;
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

  Future<bool> _write(
    String action, {
    String? id,
    String? ssid,
    String? password,
  }) async {
    final catalog = _catalog;
    if (_busy || catalog == null) return false;
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
      if (!mounted) return true;
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
      return true;
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
      return false;
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Future<bool> _confirm(String title, String body, String yes) async =>
      await showDialog<bool>(
        context: context,
        builder: (context) => AlertDialog(
          backgroundColor: CelestialColors.backgroundCard,
          title: Text(title,
              style: const TextStyle(color: CelestialColors.textPrimary)),
          content: NetworkBodyText(body),
          actions: [
            TextButton(
                onPressed: () => Navigator.pop(context, false),
                child: const Text('Cancel',
                    style: TextStyle(color: CelestialColors.textSecondary))),
            TextButton(
                style: TextButton.styleFrom(foregroundColor: networkTeal),
                onPressed: () => Navigator.pop(context, true),
                child: Text(yes)),
          ],
        ),
      ) ??
      false;

  /// A saved network is otherwise first proven by a failed pairing. Whether to
  /// save: true once the Box joined it, or the owner chose to keep it unproven.
  Future<bool> _proves(String ssid, String password) async {
    if (!await _confirm(
        'Check this network?',
        'Your Rhythm Box will join "$ssid" to prove the password, then return to its own network. It is offline for about a minute, and lights will not respond to Rhythm until it is back.',
        'Check network')) {
      return false;
    }
    if (!mounted) return false;
    setState(() {
      _busy = true;
      _error = null;
      _checking = ssid;
    });
    final operation = const Uuid().v4();
    var check = await widget.api
        .startWifiCheck(operationId: operation, ssid: ssid, password: password);
    // The Box needs up to 30 s to try and another 30 s to come home.
    final deadline = DateTime.now().add(const Duration(seconds: 100));
    while (check.state == RhythmWifiCheckState.running &&
        mounted &&
        DateTime.now().isBefore(deadline)) {
      await Future<void>.delayed(widget.checkPollInterval);
      check = await widget.api.getWifiCheck(operation) ?? check;
    }
    if (!mounted) return false;
    setState(() {
      _busy = false;
      _checking = null;
    });
    AnalyticsService().logWifiAction(
      networkChange: false,
      journeyId: _journey,
      action: 'check',
      outcome: check.state == RhythmWifiCheckState.passed
          ? 'succeeded'
          : check.state == RhythmWifiCheckState.unavailable
              ? 'cancelled'
              : 'failed',
    );
    return switch (check.state) {
      RhythmWifiCheckState.passed => true,
      // A Box that cannot check is no reason to refuse the network.
      RhythmWifiCheckState.unavailable => true,
      RhythmWifiCheckState.failed => _confirm(
          'Could not join "$ssid"',
          check.reason == 'not_found'
              ? 'Your Rhythm Box could not see this network. Check the name, and that it is a 2.4 GHz network. If it only reaches another part of your home, you can still save it.'
              : 'Your Rhythm Box could see this network but could not join it. The password is probably wrong.',
          'Save anyway'),
      RhythmWifiCheckState.running => _confirm(
          'Could not confirm "$ssid"',
          'Your Rhythm Box did not report back in time. Make sure it is back online before adding accessories.',
          'Save anyway'),
    };
  }

  /// Removing the default would otherwise leave new accessories without a
  /// network, so the owner names its successor first.
  Future<void> _remove(RhythmWifiProfile profile) async {
    final catalog = _catalog!;
    final others = catalog.profiles.where((p) => p.id != profile.id).toList();
    if (catalog.defaultId == profile.id && others.isNotEmpty) {
      final successor = await showDialog<String>(
        context: context,
        builder: (context) => SimpleDialog(
          backgroundColor: CelestialColors.backgroundCard,
          title: Text('Remove "${profile.ssid}"',
              style: const TextStyle(color: CelestialColors.textPrimary)),
          children: [
            const Padding(
                padding: EdgeInsets.fromLTRB(24, 0, 24, 8),
                child: NetworkBodyText(
                    'Choose the network new accessories should join instead.')),
            for (final other in others)
              SimpleDialogOption(
                  key: ValueKey('wifi-successor-${other.id}'),
                  onPressed: () => Navigator.pop(context, other.id),
                  child: Text(other.ssid,
                      style:
                          const TextStyle(color: CelestialColors.textPrimary))),
          ],
        ),
      );
      if (successor == null || !mounted) return;
      if (!await _write('default', id: successor)) return;
    }
    await _write('remove', id: profile.id);
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
    if (result == null || !mounted) return;
    // Keeping the stored password leaves nothing new to prove.
    final password = result.password;
    if (password != null && !await _proves(result.ssid, password)) return;
    if (mounted) {
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
                _remove(profile);
              }
            },
            itemBuilder: (_) => [
                  const PopupMenuItem(
                      value: 'edit',
                      child: Text('Edit',
                          style:
                              TextStyle(color: CelestialColors.textPrimary))),
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
          if (_checking != null)
            NetworkNotice(
                key: const ValueKey('wifi-checking'),
                text:
                    'Checking "$_checking". Your Rhythm Box is offline while it tries to join, and returns to its own network either way.'),
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
