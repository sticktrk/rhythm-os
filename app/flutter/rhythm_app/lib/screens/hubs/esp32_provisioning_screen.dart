import 'package:flutter/material.dart';

/// Bluetooth provisioning is currently disabled.
class Esp32ProvisioningScreen extends StatelessWidget {
  const Esp32ProvisioningScreen({super.key});

  static Future<void> show(BuildContext context) {
    return Navigator.of(context).push(
      MaterialPageRoute<void>(
        fullscreenDialog: true,
        builder: (_) => const Esp32ProvisioningScreen(),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('Bluetooth Provisioning')),
      body: const Center(
        child: Padding(
          padding: EdgeInsets.all(24),
          child: Text(
            'Bluetooth provisioning is currently disabled.',
            textAlign: TextAlign.center,
          ),
        ),
      ),
    );
  }
}
