import 'dart:async';
import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../providers/server_sync_provider.dart';
import 'header_close_button.dart';

/// Loads the device catalog only while a surface that uses it is visible.
class DeviceDetailsLoader extends StatefulWidget {
  final Widget child;

  /// Full routes and device sheets need navigation before their child loads.
  final bool showCloseButton;
  const DeviceDetailsLoader({
    super.key,
    required this.child,
    this.showCloseButton = false,
  });

  @override
  State<DeviceDetailsLoader> createState() => _DeviceDetailsLoaderState();
}

class _DeviceDetailsLoaderState extends State<DeviceDetailsLoader> {
  int? _requestedGeneration;
  int? _loadedOwnerGeneration;
  ServerSyncProvider? _sync;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    final sync = context.read<ServerSyncProvider>();
    if (!identical(_sync, sync)) {
      _sync?.releaseDeviceDetails();
      _sync = sync;
      _requestedGeneration = null;
      _loadedOwnerGeneration = null;
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
    final needsDetails = sync.needsDeviceDetails;
    if (!needsDetails) {
      _loadedOwnerGeneration = sync.deviceDetailsOwnerGeneration;
    } else if (_requestedGeneration != sync.deviceDetailsGeneration) {
      _load(sync);
    }
    final keepChild =
        _loadedOwnerGeneration == sync.deviceDetailsOwnerGeneration;
    return Stack(
      fit: StackFit.expand,
      children: [
        if (keepChild)
          // Keep operation contexts and local edits alive across refreshes, but
          // never carry a screen's state over to a different cache owner.
          ExcludeFocus(
            excluding: needsDetails,
            child: KeyedSubtree(
              key: ValueKey((sync, sync.deviceDetailsOwnerGeneration)),
              child: widget.child,
            ),
          ),
        if (needsDetails)
          Positioned.fill(
            child: BlockSemantics(
              child: Material(
                color: Theme.of(context)
                    .colorScheme
                    .surface
                    .withValues(alpha: keepChild ? 0.95 : 1),
                child: Stack(
                  children: [
                    Center(
                      child: sync.deviceDetailsFailed
                          ? Column(
                              mainAxisSize: MainAxisSize.min,
                              children: [
                                const Text('Could not load devices.'),
                                TextButton(
                                  onPressed: () =>
                                      unawaited(sync.ensureDeviceDetails()),
                                  child: const Text('Try again'),
                                ),
                              ],
                            )
                          : const CircularProgressIndicator(
                              semanticsLabel: 'Loading devices'),
                    ),
                    if (widget.showCloseButton)
                      Positioned(
                        top: 16,
                        right: 16,
                        child: SafeArea(
                          child: HeaderCloseButton(
                            onTap: () => Navigator.of(context).maybePop(),
                          ),
                        ),
                      ),
                  ],
                ),
              ),
            ),
          ),
      ],
    );
  }
}
