import 'dart:async';
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../providers/server_sync_provider.dart';

/// Loads the device catalog only while a surface that uses it is visible.
class DeviceDetailsLoader extends StatefulWidget {
  final Widget child;
  const DeviceDetailsLoader({super.key, required this.child});

  @override
  State<DeviceDetailsLoader> createState() => _DeviceDetailsLoaderState();
}

class _DeviceDetailsLoaderState extends State<DeviceDetailsLoader> {
  int? _requestedGeneration;
  ServerSyncProvider? _sync;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final sync = context.read<ServerSyncProvider>();
    if (!identical(_sync, sync)) {
      _sync?.releaseDeviceDetails();
      _sync = sync;
      _requestedGeneration = null;
      sync.acquireDeviceDetails();
    }
  }

  @override
  void dispose() {
    _sync?.releaseDeviceDetails();
    super.dispose();
  }

  void _load(ServerSyncProvider sync) {
    _requestedGeneration = sync.deviceDetailsGeneration;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) unawaited(sync.ensureDeviceDetails());
    });
  }

  @override
  Widget build(BuildContext context) {
    final sync = context.watch<ServerSyncProvider>();
    if (!sync.needsDeviceDetails) return widget.child;
    if (_requestedGeneration != sync.deviceDetailsGeneration) _load(sync);
    if (sync.deviceDetailsFailed) {
      return Center(
          child: Column(mainAxisSize: MainAxisSize.min, children: [
        const Text('Could not load devices.'),
        TextButton(
          onPressed: () => unawaited(sync.ensureDeviceDetails()),
          child: const Text('Try again'),
        ),
      ]));
    }
    return const Center(
        child: CircularProgressIndicator(semanticsLabel: 'Loading devices'));
  }
}
