import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../../providers/room_provider.dart';
import '../../providers/server_sync_provider.dart';
import '../../services/analytics_service.dart';
import '../../services/apple_health_sync_controller.dart';

class AppleHealthScreen extends StatelessWidget {
  const AppleHealthScreen({super.key});

  static Future<void> show(BuildContext context) {
    AnalyticsService().logScreenView('apple_health');
    return Navigator.of(context).push(
        MaterialPageRoute<void>(builder: (_) => const AppleHealthScreen()));
  }

  @override
  Widget build(BuildContext context) {
    final controller = context.watch<AppleHealthSyncController>();
    final rooms = context.watch<RoomProvider>().rooms;
    final schedules = context.watch<ServerSyncProvider>().lightSchedules;
    return Scaffold(
      appBar: AppBar(title: const Text('Apple Health')),
      body: AppleHealthControls(controller: controller, sources: {
        'global': 'Home Sleep / Wake buttons',
        for (final schedule in schedules)
          'schedule:${schedule.id}': '${schedule.name} buttons',
        for (final room in rooms.where((room) => room.kind.isRoom))
          room.id: room.name,
      }),
    );
  }
}

class AppleHealthControls extends StatefulWidget {
  final AppleHealthSyncController controller;
  final Map<String, String> sources;
  const AppleHealthControls(
      {super.key, required this.controller, required this.sources});

  @override
  State<AppleHealthControls> createState() => _AppleHealthControlsState();
}

class _AppleHealthControlsState extends State<AppleHealthControls> {
  String? _selected;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
        listenable: widget.controller,
        builder: (context, _) {
          final controller = widget.controller;
          final source = _selected ??
              (controller.matchesCurrentHome ? controller.scope?.nodeId : null);
          final validSource =
              widget.sources.containsKey(source) ? source : null;
          final environment = controller.currentEnvironment();
          final busy = controller.busy ||
              controller.status == AppleHealthSyncStatus.syncing;
          return ListView(padding: const EdgeInsets.all(20), children: [
            const Text(
                'Save your Sleep and Wake button activity as Time in Bed in Apple Health.'),
            const SizedBox(height: 12),
            const Text(
                'Choose the room or home buttons you use for your own sleep. '
                'Button presses report time in bed; they do not measure sleep or sleep stages. '
                'Only completed Sleep → Wake intervals are saved.'),
            const SizedBox(height: 24),
            DropdownButtonFormField<String>(
              key: ValueKey('apple-health-source-$validSource'),
              initialValue: validSource,
              isExpanded: true,
              decoration:
                  const InputDecoration(labelText: 'My sleep button activity'),
              items: widget.sources.entries
                  .map((entry) => DropdownMenuItem(
                      value: entry.key,
                      child:
                          Text(entry.value, overflow: TextOverflow.ellipsis)))
                  .toList(),
              onChanged: controller.enabled || busy
                  ? null
                  : (value) => setState(() => _selected = value),
            ),
            const SizedBox(height: 20),
            SwitchListTile(
              contentPadding: EdgeInsets.zero,
              title: const Text('Sync with Apple Health'),
              value: controller.enabled,
              onChanged: controller.enabled ||
                      (!busy && environment != null && validSource != null)
                  ? (enabled) {
                      if (!enabled) {
                        controller.disable();
                        return;
                      }
                      if (environment == null || validSource == null) return;
                      controller.enable(AppleHealthScope(
                          accountId: environment.accountId,
                          homeId: environment.homeId,
                          serverId: environment.serverId,
                          nodeId: validSource));
                    }
                  : null,
              subtitle: const Text(
                  'Includes retained button history. Syncs when you open '
                  'Rhythm or reconnect, and while Rhythm is active.'),
            ),
            if (environment == null)
              const Text(
                  'Connect to your Rhythm server to select your sleep activity.'),
            if (validSource == null && !controller.enabled)
              const Text('Select your button activity before enabling sync.'),
            const SizedBox(height: 16),
            Text(_status(controller), key: const Key('apple-health-status')),
            if (busy)
              const Padding(
                  padding: EdgeInsets.all(16),
                  child: LinearProgressIndicator()),
            if (controller.enabled)
              Padding(
                  padding: const EdgeInsets.only(top: 16),
                  child: FilledButton(
                      onPressed: busy || !controller.matchesCurrentHome
                          ? null
                          : controller.sync,
                      child: const Text('Sync now'))),
            const SizedBox(height: 24),
            const Text(
                'Rhythm only requests permission to write Sleep data. It does not read '
                'your Health data. Turning sync off keeps existing samples in Apple Health; '
                'you can delete them there. This setting applies only to this iPhone.'),
          ]);
        },
      );

  String _status(AppleHealthSyncController controller) {
    if (controller.enabled && !controller.matchesCurrentHome) {
      return 'Sync is paused. Return to the home and server you selected, or turn sync off to choose again.';
    }
    return switch (controller.status) {
      AppleHealthSyncStatus.disabled => 'Sync is off.',
      AppleHealthSyncStatus.ready => 'Ready to sync.',
      AppleHealthSyncStatus.syncing => 'Syncing with Apple Health…',
      AppleHealthSyncStatus.synced =>
        'Retained button history synced with Apple Health.',
      AppleHealthSyncStatus.empty =>
        'No completed Sleep → Wake intervals in the available button history.',
      AppleHealthSyncStatus.denied =>
        'Allow Rhythm to write Sleep data in Apple Health, then try again.',
      AppleHealthSyncStatus.unavailable =>
        'Apple Health is unavailable on this device.',
      AppleHealthSyncStatus.offline =>
        'Button history is unavailable. Reconnect and try again.',
      AppleHealthSyncStatus.partial =>
        'Available history synced. Some history could not be loaded; reconnect and sync again.',
      AppleHealthSyncStatus.failed =>
        'Sync could not complete. Unlock your iPhone and try again.',
    };
  }
}
