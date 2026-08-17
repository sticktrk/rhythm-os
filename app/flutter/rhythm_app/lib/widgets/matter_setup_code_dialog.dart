import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import 'solar_orbit.dart';

/// Masked-by-default owner view for retained Matter setup material.
class MatterSetupCodeDialog extends StatefulWidget {
  const MatterSetupCodeDialog({
    super.key,
    required this.secret,
    this.onCopied,
  });

  final RhythmPairingRecoverySecret secret;
  final VoidCallback? onCopied;

  @override
  State<MatterSetupCodeDialog> createState() => _MatterSetupCodeDialogState();
}

class _MatterSetupCodeDialogState extends State<MatterSetupCodeDialog> {
  bool _revealed = false;

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('Matter setup code'),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text(
            'Keep this private. Anyone with this code may be able to pair the device after it is reset.',
          ),
          const SizedBox(height: 16),
          Container(
            width: double.infinity,
            padding: const EdgeInsets.all(12),
            decoration: BoxDecoration(
              color: CelestialColors.backgroundDark,
              borderRadius: BorderRadius.circular(10),
            ),
            child: SelectableText(
              _revealed ? widget.secret.setupPayload : '•••• •••• ••••',
              key: const ValueKey('matter-setup-code-value'),
              style: const TextStyle(
                fontFamily: 'monospace',
                color: CelestialColors.textPrimary,
              ),
            ),
          ),
        ],
      ),
      actions: [
        TextButton(
          onPressed: () => setState(() => _revealed = !_revealed),
          child: Text(_revealed ? 'HIDE' : 'REVEAL'),
        ),
        TextButton(
          onPressed: () async {
            await Clipboard.setData(
              ClipboardData(text: widget.secret.setupPayload),
            );
            unawaited(
              clearMatterSetupClipboardIfUnchangedAfter(
                widget.secret.setupPayload,
                const Duration(minutes: 1),
              ),
            );
            if (!context.mounted) return;
            Navigator.of(context).pop();
            widget.onCopied?.call();
          },
          child: const Text('COPY'),
        ),
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('DONE'),
        ),
      ],
    );
  }
}

@visibleForTesting
Future<void> clearMatterSetupClipboardIfUnchangedAfter(
  String expectedSecret,
  Duration delay, {
  Future<String?> Function()? readText,
  Future<void> Function()? clear,
}) async {
  await Future<void>.delayed(delay);
  try {
    final current = await (readText ?? _readClipboardText)();
    if (current == expectedSecret) {
      await (clear ?? _clearClipboard)();
    }
  } catch (_) {
    // Clipboard expiry is best-effort and must never expose the secret through
    // an error/log path or interrupt the completed owner action.
  }
}

Future<String?> _readClipboardText() async =>
    (await Clipboard.getData(Clipboard.kTextPlain))?.text;

Future<void> _clearClipboard() =>
    Clipboard.setData(const ClipboardData(text: ''));
