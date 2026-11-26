import 'dart:async';

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_core/rhythm_core.dart';

import '../providers/home_provider.dart';
import '../providers/server_sync_provider.dart';
import '../services/assistant_attention_controller.dart';
import '../services/push_registration_service.dart';
import '../services/server_endpoint_resolver.dart';
import 'assistant_attention_sheet.dart';

class AssistantAttentionHost extends StatefulWidget {
  const AssistantAttentionHost({super.key, required this.child});
  final Widget child;

  @override
  State<AssistantAttentionHost> createState() => _AssistantAttentionHostState();
}

class _AssistantAttentionHostState extends State<AssistantAttentionHost>
    with WidgetsBindingObserver {
  final _controller = AssistantAttentionController();
  HomeProvider? _homes;
  ServerSyncProvider? _sync;
  late final StreamSubscription<String> _push;
  bool _sheetOpen = false, _queued = false;
  bool _foreground = true;

  @override
  void initState() {
    super.initState();
    final binding = WidgetsBinding.instance;
    binding.addObserver(this);
    _foreground = binding.lifecycleState == null ||
        binding.lifecycleState == AppLifecycleState.resumed;
    _controller.addListener(_scheduleSheet);
    DirectHubAccess.changes.addListener(_select);
    _push = PushRegistrationService.instance.attentionHubIds
        .listen(_controller.push);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final homes = context.read<HomeProvider>();
    final sync = context.read<ServerSyncProvider>();
    if (homes == _homes && sync == _sync) return;
    _homes?.removeListener(_select);
    _sync?.removeListener(_select);
    _homes = homes..addListener(_select);
    _sync = sync..addListener(_select);
    _select();
    final pending =
        PushRegistrationService.instance.takePendingAttentionHubId();
    if (pending != null) _controller.push(pending);
  }

  void _select() {
    final homes = _homes;
    final sync = _sync;
    if (homes == null || sync == null || _controller.isCurrent) return;
    final hub = homes.activeServerHub;
    final home = homes.currentHome?.id;
    final instance = sync.connectedServerInstanceId;
    if (hub == null ||
        home == null ||
        instance == null ||
        !sync.connection.connected ||
        sync.connectedServerHub?.id != hub.id ||
        sync.activeConnectionEndpoint != hub.endpoint) {
      if (_controller.hasTarget) _controller.clear();
      return;
    }
    final selection = DirectHubAccess.capture();
    final connection = sync.connection;
    _controller.select(
        hubId: hub.id,
        api: ServerEndpointResolver.local(hub).authApi(),
        attentionEvents: connection.assistantAttentionEvents,
        isCurrentTarget: () =>
            mounted &&
            _homes == homes &&
            _sync == sync &&
            selection.isSameSelection &&
            homes.currentHome?.id == home &&
            homes.activeServerHub?.id == hub.id &&
            homes.activeServerHub?.token == hub.token &&
            homes.activeServerHub?.endpoint == hub.endpoint &&
            identical(sync.connection, connection) &&
            connection.connected &&
            sync.connectedServerInstanceId == instance &&
            sync.connectedServerHub?.id == hub.id &&
            sync.activeConnectionEndpoint == hub.endpoint);
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    _foreground = state == AppLifecycleState.resumed;
    if (_foreground) {
      _select();
      unawaited(_controller.refresh());
    }
  }

  void _scheduleSheet() {
    if (_queued ||
        _sheetOpen ||
        !_foreground ||
        !mounted ||
        _controller.next == null) {
      return;
    }
    _queued = true;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _queued = false;
      if (mounted) unawaited(_show());
    });
    WidgetsBinding.instance.ensureVisualUpdate();
  }

  Future<void> _show() async {
    final request = _controller.next;
    final api = _controller.api;
    if (request == null || api == null || _sheetOpen || !_foreground) return;
    _sheetOpen = true;
    try {
      if (request is ConnectionAttention) {
        unawaited(PushRegistrationService.instance.requestForAssistant());
      }
      await showAssistantAttentionSheet(context,
          request: request,
          api: api,
          isCurrentTarget: () =>
              _controller.api == api && _controller.contains(request.id),
          changes: _controller);
    } finally {
      _sheetOpen = false;
      if (mounted && _controller.api == api) {
        _controller.dismiss(request.id);
        unawaited(_controller.refresh());
      }
      if (mounted) _scheduleSheet();
    }
  }

  @override
  void dispose() {
    _push.cancel();
    WidgetsBinding.instance.removeObserver(this);
    _homes?.removeListener(_select);
    _sync?.removeListener(_select);
    DirectHubAccess.changes.removeListener(_select);
    _controller.removeListener(_scheduleSheet);
    _controller.clear();
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) =>
      ChangeNotifierProvider<AssistantAttentionController>.value(
          value: _controller, child: widget.child);
}
