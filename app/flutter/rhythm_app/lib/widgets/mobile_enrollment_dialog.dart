import 'dart:async';

import 'package:flutter/material.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';
import 'package:uuid/uuid.dart';

import '../services/analytics_service.dart';

/// Enrollment approval stays in memory until exchanged and is never logged.
class MobileEnrollmentDialog extends StatefulWidget {
  const MobileEnrollmentDialog({super.key, required this.exchange});

  final Future<RhythmOwnerClaim> Function(RhythmMobileEnrollment) exchange;

  @override
  State<MobileEnrollmentDialog> createState() => _MobileEnrollmentDialogState();
}

class _MobileEnrollmentDialogState extends State<MobileEnrollmentDialog> {
  final _controller = TextEditingController();
  final _journeyId = 'mobile-enrollment-${const Uuid().v4()}';
  bool _busy = false;
  String? _error;

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  Future<void> _connect() async {
    if (_busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    var failureStage = 'validation';
    try {
      final enrollment = RhythmMobileEnrollment.parse(_controller.text.trim());
      if (enrollment.isExpired()) {
        throw const FormatException(
          'Create a fresh connection code in Home Assistant.',
        );
      }
      failureStage = 'exchange';
      final claim = await widget.exchange(enrollment);
      unawaited(AnalyticsService().logMobileEnrollmentCompleted(
        journeyId: _journeyId,
        outcome: 'succeeded',
      ));
      if (!mounted) return;
      _controller.clear();
      Navigator.of(context).pop(claim.token);
    } catch (error) {
      unawaited(AnalyticsService().logMobileEnrollmentCompleted(
        journeyId: _journeyId,
        outcome: 'failed',
        failureStage: failureStage,
      ));
      if (!mounted) return;
      setState(() {
        _busy = false;
        _error = error is FormatException
            ? error.message
            : 'Connection failed. Create a new connection code in Home Assistant and try again.';
      });
    }
  }

  @override
  Widget build(BuildContext context) => PopScope(
        canPop: !_busy,
        child: AlertDialog(
          title: const Text('Connect mobile app'),
          content: SingleChildScrollView(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                const Text(
                  'In Home Assistant, open Rhythm and choose Connect mobile app. '
                  'Paste the complete connection code here. It works once and expires after a few minutes.',
                ),
                const SizedBox(height: 16),
                TextField(
                  key: const ValueKey('mobile-enrollment-code'),
                  controller: _controller,
                  enabled: !_busy,
                  autocorrect: false,
                  enableSuggestions: false,
                  obscureText: true,
                  decoration: InputDecoration(
                    labelText: 'Connection code',
                    errorText: _error,
                    errorMaxLines: 3,
                  ),
                  onSubmitted: (_) => _connect(),
                ),
              ],
            ),
          ),
          actions: [
            TextButton(
              onPressed: _busy ? null : () => Navigator.of(context).pop(),
              child: const Text('Cancel'),
            ),
            FilledButton(
              key: const ValueKey('mobile-enrollment-connect'),
              onPressed: _busy ? null : _connect,
              child: Text(_busy ? 'Connecting…' : 'Connect'),
            ),
          ],
        ),
      );
}
