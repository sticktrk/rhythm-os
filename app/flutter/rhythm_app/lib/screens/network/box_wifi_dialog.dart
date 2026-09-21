import 'package:flutter/material.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../../widgets/solar_orbit.dart';
import 'network_ui.dart';

/// Where the owner wants the Rhythm Box to move: a saved network, whose
/// password never leaves the Box, or a newly typed one.
class BoxWifiChoice {
  const BoxWifiChoice.saved(
      {required String this.profileId, required this.ssid})
      : password = null;
  const BoxWifiChoice.typed({required this.ssid, required String this.password})
      : profileId = null;
  final String? profileId;
  final String ssid;
  final String? password;
}

class BoxWifiDialog extends StatefulWidget {
  /// [catalog] is null when the Box has no saved-network support or the
  /// catalog could not be read; the dialog then only offers typed entry.
  const BoxWifiDialog({super.key, this.catalog});
  final RhythmWifiProfiles? catalog;

  @override
  State<BoxWifiDialog> createState() => _BoxWifiDialogState();
}

class _BoxWifiDialogState extends State<BoxWifiDialog> {
  static const _other = '';
  final _ssid = TextEditingController();
  final _password = TextEditingController();
  late String? _selection = _choices.isEmpty ? _other : null;

  /// The network the Box is already on is not a destination.
  List<RhythmWifiProfile> get _choices => [
        for (final profile
            in widget.catalog?.profiles ?? const <RhythmWifiProfile>[])
          if (profile.id != widget.catalog?.boxProfileId) profile
      ];

  @override
  void dispose() {
    _ssid.dispose();
    _password.dispose();
    super.dispose();
  }

  BoxWifiChoice? get _choice {
    final selection = _selection;
    if (selection == null) return null;
    if (selection == _other) {
      final ssid = _ssid.text.trim();
      return ssid.isEmpty
          ? null
          : BoxWifiChoice.typed(ssid: ssid, password: _password.text);
    }
    final profile = _choices.firstWhere((p) => p.id == selection);
    return BoxWifiChoice.saved(profileId: profile.id, ssid: profile.ssid);
  }

  void _submit() {
    final choice = _choice;
    if (choice != null) Navigator.of(context, rootNavigator: true).pop(choice);
  }

  Widget _option(String value, String title,
      {IconData icon = Icons.wifi_rounded}) {
    final selected = _selection == value;
    return InkWell(
      key: ValueKey('box-wifi-option-${value.isEmpty ? 'other' : value}'),
      borderRadius: BorderRadius.circular(10),
      onTap: () => setState(() => _selection = value),
      child: Padding(
        padding: const EdgeInsets.symmetric(vertical: 10, horizontal: 4),
        child: Row(children: [
          Icon(icon,
              size: 20,
              color: selected ? networkTeal : CelestialColors.textSecondary),
          const SizedBox(width: 14),
          Expanded(
            child: Text(title,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: const TextStyle(
                    color: CelestialColors.textPrimary, fontSize: 15)),
          ),
          Icon(
            selected
                ? Icons.radio_button_checked_rounded
                : Icons.radio_button_off_rounded,
            size: 22,
            color: selected
                ? networkTeal
                : CelestialColors.textSecondary.withValues(alpha: 0.5),
          ),
        ]),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final choices = _choices;
    final typed = _selection == _other;
    return AlertDialog(
      backgroundColor: CelestialColors.backgroundCard,
      title: const Text('Change Wi-Fi',
          style: TextStyle(color: CelestialColors.textPrimary)),
      content: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            if (choices.isNotEmpty) ...[
              const NetworkBodyText(
                  'Move your Rhythm Box to a saved network or enter another one.'),
              const SizedBox(height: 8),
              for (final profile in choices) _option(profile.id, profile.ssid),
              _option(_other, 'Other network', icon: Icons.add_rounded),
            ],
            if (typed) ...[
              if (choices.isNotEmpty) const SizedBox(height: 8),
              TextField(
                key: const ValueKey('box-wifi-ssid'),
                controller: _ssid,
                autofocus: true,
                autocorrect: false,
                enableSuggestions: false,
                textInputAction: TextInputAction.next,
                onChanged: (_) => setState(() {}),
                style: const TextStyle(color: CelestialColors.textPrimary),
                decoration: const InputDecoration(
                  labelText: 'Network name',
                  prefixIcon: Icon(Icons.wifi_rounded),
                ),
              ),
              const SizedBox(height: 12),
              TextField(
                key: const ValueKey('box-wifi-password'),
                controller: _password,
                obscureText: true,
                autocorrect: false,
                enableSuggestions: false,
                textInputAction: TextInputAction.done,
                onSubmitted: (_) => _submit(),
                style: const TextStyle(color: CelestialColors.textPrimary),
                decoration: const InputDecoration(
                  labelText: 'Password',
                  prefixIcon: Icon(Icons.lock_outline_rounded),
                ),
              ),
            ],
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context, rootNavigator: true).pop(),
          child: const Text('Cancel',
              style: TextStyle(color: CelestialColors.textSecondary)),
        ),
        TextButton(
          key: const ValueKey('box-wifi-change'),
          style: TextButton.styleFrom(foregroundColor: networkTeal),
          onPressed: _choice == null ? null : _submit,
          child: const Text('Change'),
        ),
      ],
    );
  }
}
