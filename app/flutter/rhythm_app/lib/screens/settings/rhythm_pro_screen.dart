import 'package:dio/dio.dart';
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../../backend/auth/auth_user.dart';
import '../../config/feature_flags.dart';
import '../../providers/home_provider.dart';
import '../../providers/server_sync_provider.dart';
import '../../services/auth_service.dart';
import '../../services/remote_access_service.dart';
import '../../services/server_endpoint_resolver.dart';
import '../../widgets/remote_tunnel_tile.dart';
import '../../widgets/assistant_chat_widgets.dart';
import '../../widgets/settings_row.dart';
import '../../widgets/solar_orbit.dart' show CelestialColors;
import '../hubs/assistant_access_screen.dart';
import '../hubs/lighting_chat_screen.dart';
import 'dialogs/sign_in_modal.dart';
import 'sections/rhythm_server_section.dart';

const _hubTeal = Color(0xFF00BCD4);
const _tunnelGreen = Color(0xFF26A69A);
const _profileAmber = Color(0xFFFFC857);
const _agentSlate = Color(0xFF607D8B);

class RhythmProScreen extends StatelessWidget {
  const RhythmProScreen({super.key});

  @override
  Widget build(BuildContext context) {
    final homes = context.watch<HomeProvider>();
    final sync = context.watch<ServerSyncProvider>();
    final hub = homes.activeServerHub;
    final home = homes.currentHome;
    final connected = hub != null &&
        sync.connection.connected &&
        sync.connectedServerHub?.id == hub.id &&
        sync.connectedServerInstanceId != null;
    return RhythmAssistantPage(
      title: 'Rhythm Pro',
      body: StreamBuilder<AuthUser?>(
        stream: AuthService().authStateChanges,
        initialData: AuthService().currentUser,
        builder: (context, snapshot) {
          final user = snapshot.data;
          final signedIn = user != null && !user.isAnonymous;
          return ListView(
              padding: const EdgeInsets.fromLTRB(20, 8, 20, 24),
              children: [
                const SettingsSectionHeader(title: 'Free during preview'),
                SettingsGroup(children: [
                  SettingsRow(
                    icon: Icons.chat_bubble_outline_rounded,
                    iconColor: CelestialColors.sunWarm,
                    label: 'Lighting assistant',
                    onTap: signedIn || FeatureFlags.auxSignIn
                        ? () => LightingChatScreen.show(context)
                        : null,
                  ),
                  SettingsRow(
                      icon: Icons.account_circle_outlined,
                      iconColor: CelestialColors.accentBlue,
                      label: signedIn
                          ? 'Rhythm account'
                          : 'Connect your Rhythm account',
                      subtitle: signedIn
                          ? user.email ?? 'Signed in'
                          : 'Sign in to use lighting chat and remote access.',
                      showChevron: !signedIn && FeatureFlags.auxSignIn,
                      onTap: !signedIn && FeatureFlags.auxSignIn
                          ? () => SignInModal.show(context)
                          : null),
                ]),
                const SizedBox(height: 20),
                SettingsGroup(children: [
                  SettingsRow(
                      icon: Icons.developer_board,
                      iconColor: _hubTeal,
                      label: hub?.name ?? 'Connect a hub',
                      subtitle: hub == null
                          ? 'Pair your LightBox or Home Assistant add-on.'
                          : home?.name ?? 'Selected hub',
                      onTap: () => RhythmServerDetailScreen.show(context)),
                  if (hub != null)
                    const RemoteTunnelTile()
                  else
                    const SettingsRow(
                        icon: Icons.public,
                        iconColor: _tunnelGreen,
                        label: 'Remote access tunnel',
                        subtitle: 'Pair a hub to set up access away from home.',
                        showChevron: false),
                  if (hub != null && connected)
                    RhythmProHubControls(
                      key: ValueKey((
                        home?.id,
                        hub.id,
                        sync.connectedServerInstanceId,
                        user?.id
                      )),
                      hub: hub,
                      isLocal: sync.activeConnectionEndpoint == hub.endpoint,
                    )
                  else ...[
                    const SettingsRow(
                        icon: Icons.cloud_download_outlined,
                        iconColor: _profileAmber,
                        label: 'Bulb profile updates',
                        subtitle:
                            'Connect locally to manage color-matching updates.',
                        showChevron: false),
                    const SettingsRow(
                        icon: Icons.link,
                        iconColor: _agentSlate,
                        label: 'Remote MCP',
                        subtitle:
                            'Connect locally to manage external agent access.',
                        showChevron: false),
                  ],
                ]),
              ]);
        },
      ),
    );
  }
}

class RhythmProHubControls extends StatefulWidget {
  const RhythmProHubControls(
      {super.key, required this.hub, this.api, required this.isLocal});
  final Hub hub;
  final RhythmAuthApi? api;
  final bool isLocal;
  @override
  State<RhythmProHubControls> createState() => _RhythmProHubControlsState();
}

class _RhythmProHubControlsState extends State<RhythmProHubControls> {
  RhythmProSettings? _settings;
  String? _error;
  bool _busy = false;
  bool _ownerRequired = false;
  int _request = 0;

  @override
  void initState() {
    super.initState();
    _load();
  }

  @override
  void didUpdateWidget(RhythmProHubControls old) {
    super.didUpdateWidget(old);
    if (widget.isLocal != old.isLocal ||
        widget.hub.token != old.hub.token ||
        widget.hub.endpoint != old.hub.endpoint) {
      _load();
    }
  }

  Future<void> _load({bool? updates, bool? remoteMcp}) async {
    final request = ++_request;
    if (!widget.isLocal) {
      setState(() {
        _settings = null;
        _busy = false;
        _error = 'Connect locally to manage these hub settings.';
      });
      return;
    }
    final api =
        widget.api ?? ServerEndpointResolver.local(widget.hub).authApi();
    setState(() {
      _busy = true;
      _error = null;
      _ownerRequired = false;
      if (updates == null && remoteMcp == null) _settings = null;
    });
    try {
      final settings = updates == null && remoteMcp == null
          ? await api.getProSettings()
          : await api.updateProSettings(
              bulbProfileUpdates: updates, remoteMcp: remoteMcp);
      if (mounted && request == _request) setState(() => _settings = settings);
    } catch (error) {
      final status = error is DioException ? error.response?.statusCode : null;
      if (mounted && request == _request) {
        setState(() {
          _settings = null;
          _ownerRequired = status == 401 || status == 403;
          _error = switch (status) {
            404 =>
              'Update this hub to manage remote MCP and bulb profile updates.',
            401 ||
            403 =>
              'This hub requires a locally connected owner to manage Pro settings.',
            _ => 'Could not verify the hub settings. Refresh to try again.',
          };
        });
      }
    } finally {
      if (mounted && request == _request) setState(() => _busy = false);
    }
  }

  Future<void> _connectOwner() async {
    final hub = widget.hub;
    if (hub.token?.isNotEmpty == true) {
      await RhythmServerDetailScreen.show(context);
      if (mounted) await _load();
      return;
    }
    final homes = context.read<HomeProvider>();
    final selection = DirectHubAccess.capture();
    setState(() => _busy = true);
    try {
      final owner =
          await RemoteAccessService.instance.ensureOwnerTokenForHub(hub);
      if (!mounted ||
          !selection.isSameSelection ||
          homes.activeServerHub?.id != hub.id) {
        return;
      }
      if (!await homes.updateHub(owner)) {
        throw StateError('Could not save the owner credential.');
      }
    } catch (_) {
      if (mounted) await RhythmServerDetailScreen.show(context);
    } finally {
      if (mounted) await _load();
    }
  }

  void _credentials() {
    final homes = context.read<HomeProvider>();
    final sync = context.read<ServerSyncProvider>();
    final hub = widget.hub;
    final homeId = homes.currentHome?.id;
    final identity = sync.connectedServerInstanceId;
    final selection = DirectHubAccess.capture();
    bool sameTarget() =>
        mounted &&
        selection.isSameSelection &&
        homes.currentHome?.id == homeId &&
        homes.activeServerHub?.id == hub.id &&
        sync.connection.connected &&
        sync.connectedServerHub?.id == hub.id &&
        sync.connectedServerInstanceId == identity &&
        sync.activeConnectionEndpoint == hub.endpoint;
    if (!sameTarget()) return;
    final local = ServerEndpointResolver.local(hub);
    final endpoint = hub.remoteEndpoint;
    final url = endpoint == null ? local.baseUrl : endpoint.baseUrl;
    Navigator.of(context).push(MaterialPageRoute(
        builder: (_) => AssistantAccessScreen(
            api: local.authApi(),
            attentionEvents: sync.connection.assistantAttentionEvents,
            endpoint:
                Uri.parse('$url/').resolve('api/assistant/mcp').toString(),
            isCurrentTarget: sameTarget,
            targetChanges:
                Listenable.merge([homes, sync, DirectHubAccess.changes]))));
  }

  @override
  Widget build(BuildContext context) {
    final settings = _settings;
    if (settings == null) {
      return SettingsRow(
          icon: Icons.tune_rounded,
          iconColor: _agentSlate,
          label: 'Remote MCP and bulb profile updates',
          subtitle: _error ?? 'Reading hub settings…',
          showChevron: false,
          trailing: _ownerRequired && !_busy && widget.isLocal
              ? TextButton(
                  onPressed: _connectOwner,
                  child: const Text('Connect as owner'))
              : !_busy && widget.isLocal
                  ? IconButton(
                      icon: const Icon(Icons.refresh),
                      color: CelestialColors.textSecondary,
                      tooltip: 'Refresh hub settings',
                      onPressed: () => _load())
                  : null);
    }
    void Function(bool)? when(void Function(bool) change) =>
        _busy ? null : change;
    final updates = when((v) => _load(updates: v));
    final remoteMcp = when((v) => _load(remoteMcp: v));
    // These rows sit inside the screen's group alongside its other rows, so
    // they carry the group's dividers themselves.
    return Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
      SettingsRow(
          icon: Icons.cloud_download_outlined,
          iconColor: _profileAmber,
          label: 'Bulb profile updates',
          subtitle:
              'Download color-matching updates. Saved profiles work offline.',
          showChevron: false,
          trailing: SettingsSwitch(
              value: settings.bulbProfileUpdates, onChanged: updates),
          onTap:
              updates == null ? null : () => updates(!settings.bulbProfileUpdates)),
      const SettingsDivider(),
      SettingsRow(
          icon: Icons.link,
          iconColor: _agentSlate,
          label: 'Remote MCP',
          subtitle: widget.hub.remoteEndpoint == null
              ? 'External agents need an active tunnel and an agent credential.'
              : 'Let authorized agents connect through the tunnel.',
          showChevron: false,
          trailing:
              SettingsSwitch(value: settings.remoteMcp, onChanged: remoteMcp),
          onTap: remoteMcp == null ? null : () => remoteMcp(!settings.remoteMcp)),
      if (settings.remoteMcp) ...[
        const SettingsDivider(),
        SettingsRow(
            icon: Icons.key,
            iconColor: CelestialColors.accentBlue,
            label: 'Manage agent credentials',
            onTap: _credentials),
      ],
    ]);
  }
}
