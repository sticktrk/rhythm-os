import 'package:flutter/material.dart';
import 'package:provider/provider.dart';
import '../providers/server_sync_provider.dart';
import '../screens/hubs/ha_matter_management_screen.dart';
import 'package:url_launcher/url_launcher.dart';

/// Device commissioning and area membership belong to Home Assistant.
/// Returning from this handoff is not evidence that a device was commissioned.
class HomeAssistantDeviceSetup extends StatelessWidget {
  const HomeAssistantDeviceSetup({super.key});

  static Future<void> show(BuildContext context) => showDialog<void>(
        context: context,
        builder: (_) => const AlertDialog(content: HomeAssistantDeviceSetup()),
      );

  Future<void> _open(BuildContext context) async {
    var opened = false;
    try {
      opened = await launchUrl(
        Uri.parse('https://my.home-assistant.io/redirect/integrations/'),
        mode: LaunchMode.externalApplication,
      );
    } catch (_) {
      // The instructions remain useful without an installed browser or HA app.
    }
    if (!opened && context.mounted) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(
          content: Text(
            'Open Home Assistant, then Settings → Devices & services.',
          ),
        ),
      );
    }
  }

  @override
  Widget build(BuildContext context) => Column(
        key: const ValueKey('home-assistant-device-setup'),
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          const Icon(Icons.home_outlined, size: 40),
          const SizedBox(height: 16),
          Text(
            'Set up devices in Home Assistant',
            style: Theme.of(context).textTheme.titleLarge,
          ),
          const SizedBox(height: 12),
          const Text(
            'Add lights, buttons, and sensors in Home Assistant under Settings → '
            'Devices & services. Assign their areas there. Then open Rhythm in '
            'Home Assistant to review which lights Rhythm may control. '
            'Approved devices will appear here when synchronized.',
          ),
          const SizedBox(height: 20),
          if (context.watch<ServerSyncProvider>().canManageHaMatter) ...[
            FilledButton(
              onPressed: () => HaMatterManagementScreen.show(context),
              child: const Text('Manage Matter devices in Rhythm'),
            ),
            const SizedBox(height: 12),
          ],
          FilledButton(
            onPressed: () => _open(context),
            child: const Text('Open Home Assistant'),
          ),
        ],
      );
}
