import 'package:flutter/material.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../services/phone_ble_wifi_service.dart';

/// Only used when the Box explicitly reports that it has no saved credentials.
class CommissioningWifiDialog extends StatefulWidget {
  const CommissioningWifiDialog({super.key, required this.validateWifi});
  final void Function(RhythmCommissioningWifi) validateWifi;
  @override
  State<CommissioningWifiDialog> createState() =>
      _CommissioningWifiDialogState();
}

class _CommissioningWifiDialogState extends State<CommissioningWifiDialog> {
  final _ssid = TextEditingController();
  final _password = TextEditingController();
  String? _error;

  @override
  void dispose() {
    _ssid.clear();
    _password.clear();
    _ssid.dispose();
    _password.dispose();
    super.dispose();
  }

  void _submit() {
    final wifi =
        RhythmCommissioningWifi(ssid: _ssid.text, password: _password.text);
    try {
      widget.validateWifi(wifi);
      Navigator.of(context).pop(wifi);
    } on PhoneBleWifiFailure catch (error) {
      setState(() => _error = error.message);
    }
  }

  @override
  Widget build(BuildContext context) => AlertDialog(
        title: const Text('Join your Wi-Fi'),
        content: SingleChildScrollView(
            child: Column(mainAxisSize: MainAxisSize.min, children: [
          const Text(
              'Your Rhythm Box has no saved Wi-Fi details. Enter a 2.4 GHz network that can reach the Box.'),
          const SizedBox(height: 16),
          TextField(
              key: const ValueKey('commissioning-wifi-ssid'),
              controller: _ssid,
              autocorrect: false,
              enableSuggestions: false,
              decoration: const InputDecoration(labelText: 'Wi-Fi name')),
          TextField(
              key: const ValueKey('commissioning-wifi-password'),
              controller: _password,
              obscureText: true,
              autocorrect: false,
              enableSuggestions: false,
              decoration: const InputDecoration(
                  labelText: 'Wi-Fi password',
                  helperText: 'Leave empty only for an open network')),
          if (_error != null)
            Text(_error!,
                style: TextStyle(color: Theme.of(context).colorScheme.error)),
        ])),
        actions: [
          TextButton(
              onPressed: () => Navigator.of(context).pop(),
              child: const Text('Cancel')),
          FilledButton(onPressed: _submit, child: const Text('Continue')),
        ],
      );
}
