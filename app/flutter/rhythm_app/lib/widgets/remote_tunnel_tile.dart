import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../providers/home_provider.dart';
import '../providers/server_sync_provider.dart';
import '../services/remote_access_service.dart';

class RemoteTunnelTile extends StatefulWidget {
  const RemoteTunnelTile({super.key});
  @override
  State<RemoteTunnelTile> createState() => _RemoteTunnelTileState();
}

class _RemoteTunnelTileState extends State<RemoteTunnelTile> {
  bool _isTogglingRemoteAccess = false;
  (String, String, String?, String?)? _statusKey;
  Future<RhythmRemoteAccessStatus>? _status;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final hub = context.watch<HomeProvider>().activeServerHub;
    final sync = context.watch<ServerSyncProvider>();
    final endpoint =
        sync.connection.connected && sync.connectedServerHub?.id == hub?.id
            ? sync.activeConnectionEndpoint
            : null;
    final key = hub != null && endpoint != null
        ? (hub.id, endpoint.baseUrl, hub.token, sync.connectedServerInstanceId)
        : null;
    if (key == _statusKey) return;
    _statusKey = key;
    _status = key == null
        ? null
        : RhythmRemoteAccessApi(baseUrl: key.$2, authToken: key.$3).getStatus();
  }

  void _refresh() {
    final key = _statusKey;
    if (key != null) {
      setState(() {
        _status = RhythmRemoteAccessApi(baseUrl: key.$2, authToken: key.$3)
            .getStatus();
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final hub = context.watch<HomeProvider>().activeServerHub;
    final available = hub != null && _canUseRemoteAccess();
    return FutureBuilder<RhythmRemoteAccessStatus>(
        key: ObjectKey(_status),
        future: _status,
        builder: (context, snapshot) {
          final reading = snapshot.connectionState == ConnectionState.waiting;
          final status = snapshot.data;
          final savedRoute = hub?.remoteEndpoint != null;
          final description = switch ((status, savedRoute)) {
            (final s?, _) when !s.enabled && s.configured =>
              'Paused on the hub.',
            (final s?, _) when s.enabled && !s.connectorHealthy =>
              'Enabled on the hub. Waiting for the tunnel to connect.',
            (null, true) =>
              'This phone has a saved remote connection. Hub status is unavailable.',
            (null, false) when snapshot.hasError =>
              'Could not read tunnel status from the hub.',
            _ => available
                ? 'Use the Rhythm app away from home.'
                : 'Sign in to your Rhythm account to set up remote access.',
          };
          return SwitchListTile(
            secondary: const Icon(Icons.public),
            title: const Text('Remote access tunnel'),
            subtitle: Text(_isTogglingRemoteAccess
                ? 'Updating…'
                : reading
                    ? 'Reading tunnel status…'
                    : description),
            value: status?.enabled ?? savedRoute,
            onChanged: available && !_isTogglingRemoteAccess && !reading
                ? _setRemoteAccessEnabled
                : null,
          );
        });
  }

  bool _canUseRemoteAccess() {
    try {
      return RemoteAccessService.instance.canUseRemoteAccess;
    } catch (error) {
      debugPrint('Remote access unavailable: $error');
      return false;
    }
  }

  Future<void> _setRemoteAccessEnabled(bool enabled) async {
    if (_isTogglingRemoteAccess) return;

    final homeProvider = context.read<HomeProvider>();
    final syncProvider = context.read<ServerSyncProvider>();
    final home = homeProvider.currentHome;
    final serverHub = homeProvider.activeServerHub;
    if (home == null || serverHub == null) {
      _showSnackBar(
        'No active server is available.',
        backgroundColor: Colors.red.shade400,
      );
      return;
    }

    setState(() => _isTogglingRemoteAccess = true);
    try {
      final service = RemoteAccessService.instance;
      Hub updatedHub;
      var routeVerified = true;
      if (enabled) {
        final tokenHub = await service.ensureOwnerTokenForHub(serverHub);
        if (homeProvider.currentHome?.id != home.id ||
            homeProvider.activeServerHub?.id != serverHub.id) {
          throw StateError('The selected hub changed.');
        }
        if (tokenHub.token != serverHub.token) {
          final saved = await homeProvider.updateHub(tokenHub);
          if (!saved) {
            throw StateError('Could not save the server owner token.');
          }
          if (homeProvider.currentHome?.id != home.id ||
              homeProvider.activeServerHub?.id != serverHub.id) {
            throw StateError('The selected hub changed.');
          }
        }

        final result = await service.enableForHub(
          tokenHub,
          home: home,
          explicitUserEnable: true,
        );
        updatedHub = result.updatedHub;
        routeVerified = result.routeVerified;
      } else {
        updatedHub = await service.disableForHub(serverHub, home: home);
      }

      final saved = await homeProvider.updateHub(
        updatedHub,
        clearCloudRemoteEndpoint: !enabled,
      );
      if (!saved) {
        throw StateError('Could not save the remote access setting.');
      }

      syncProvider.connectIfAvailable();
      if (enabled && !routeVerified) {
        service.scheduleAutoEnableForHub(
          home: home,
          serverHub: updatedHub,
          saveHub: homeProvider.updateHub,
          resolveLatestHub: (homeId, hubId) {
            if (homeProvider.currentHome?.id != homeId) return null;
            for (final hub in homeProvider.currentHomeHubs) {
              if (hub.id == hubId) return hub;
            }
            return null;
          },
          onEnabled: (_) => syncProvider.connectIfAvailable(),
        );
      }
      _showSnackBar(
        enabled
            ? routeVerified
                ? 'Remote access enabled.'
                : 'Remote access is starting in the background.'
            : 'Remote access disabled.',
      );
    } catch (error, stackTrace) {
      debugPrint('Remote access toggle failed: $error');
      debugPrint('$stackTrace');
      _showSnackBar(
        enabled
            ? 'Could not enable remote access.'
            : 'Could not disable remote access.',
        backgroundColor: Colors.red.shade400,
      );
    } finally {
      if (mounted) {
        setState(() => _isTogglingRemoteAccess = false);
        _refresh();
      }
    }
  }

  void _showSnackBar(String message, {Color? backgroundColor}) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text(message),
        behavior: SnackBarBehavior.floating,
        backgroundColor: backgroundColor,
      ),
    );
  }
}
