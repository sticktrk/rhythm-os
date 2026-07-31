import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../../providers/server_sync_provider.dart';
import '../../services/analytics_service.dart';
import '../../services/device_pairing_code.dart';
import '../../widgets/solar_orbit.dart';

typedef AidotButtonPairingRequest = Future<Map<String, dynamic>?> Function({
  required String hubType,
  required Map<String, dynamic> params,
  required Duration receiveTimeout,
  required String sessionId,
});

class AidotButtonAddScreen extends StatefulWidget {
  const AidotButtonAddScreen({
    super.key,
    required this.setupPayload,
    this.analyticsSource = 'unknown',
    required this.journeyId,
    @visibleForTesting this.pairingRequest,
  });

  final String setupPayload;
  final String analyticsSource;
  final String journeyId;
  final AidotButtonPairingRequest? pairingRequest;

  static Future<RhythmPairedDevice?> show(
    BuildContext context, {
    required String setupPayload,
    required String analyticsSource,
    required String journeyId,
  }) {
    return Navigator.of(context).push<RhythmPairedDevice>(
      MaterialPageRoute(
        builder: (_) => AidotButtonAddScreen(
          setupPayload: setupPayload,
          analyticsSource: analyticsSource,
          journeyId: journeyId,
        ),
      ),
    );
  }

  @override
  State<AidotButtonAddScreen> createState() => _AidotButtonAddScreenState();
}

class _AidotButtonAddScreenState extends State<AidotButtonAddScreen> {
  static const _accent = Color(0xFF26A69A);
  static const _timeout = Duration(seconds: 75);

  bool _pairing = false;
  String? _error;
  int _attemptNumber = 0;

  AidotButtonSetupCode? get _setup =>
      AidotButtonSetupCode.tryParse(widget.setupPayload);

  Future<Map<String, dynamic>?> _pair(String sessionId) {
    final injected = widget.pairingRequest;
    if (injected != null) {
      return injected(
        hubType: 'aidot_ble',
        params: {'setup_payload': widget.setupPayload},
        receiveTimeout: _timeout,
        sessionId: sessionId,
      );
    }
    return context.read<ServerSyncProvider>().api.pairDevice(
          hubType: 'aidot_ble',
          params: {'setup_payload': widget.setupPayload},
          receiveTimeout: _timeout,
          sessionId: sessionId,
        );
  }

  Future<void> _startPairing() async {
    if (_pairing || _setup == null) return;
    final attempt = ++_attemptNumber;
    final sessionId = attempt == 1
        ? widget.journeyId
        : '${widget.journeyId}-attempt-$attempt';
    setState(() {
      _pairing = true;
      _error = null;
    });
    HapticFeedback.mediumImpact();
    AnalyticsService().logAidotButtonPairingAttempted(
      journeyId: widget.journeyId,
      source: widget.analyticsSource,
      inputMethod: 'qr_code',
      attemptNumber: attempt,
    );

    try {
      final response = await _pair(sessionId);
      if (!mounted) return;
      final status = response?['status']?.toString();
      final rawDevice = response?['device'];
      if (status != 'complete' || rawDevice is! Map) {
        final message = response?['error']?.toString().trim();
        _fail(
          message?.isNotEmpty == true
              ? message!
              : 'The Rhythm Box could not find this button. Put it back in '
                  'pairing mode and try again.',
          attempt: attempt,
          failureStage: response == null ? 'empty_response' : 'pairing',
        );
        return;
      }
      final device = RhythmPairedDevice.fromJson(
        rawDevice.cast<String, dynamic>(),
      );
      AnalyticsService().logAidotButtonPairingCompleted(
        journeyId: widget.journeyId,
        source: widget.analyticsSource,
        inputMethod: 'qr_code',
        attemptNumber: attempt,
        outcome: 'succeeded',
      );
      HapticFeedback.heavyImpact();
      if (mounted) Navigator.of(context).pop(device);
    } catch (_) {
      if (!mounted) return;
      _fail(
        'Could not reach the Rhythm Box. Check its connection and try again.',
        attempt: attempt,
        failureStage: 'request_exception',
      );
    }
  }

  void _fail(
    String message, {
    required int attempt,
    required String failureStage,
  }) {
    AnalyticsService().logAidotButtonPairingCompleted(
      journeyId: widget.journeyId,
      source: widget.analyticsSource,
      inputMethod: 'qr_code',
      attemptNumber: attempt,
      outcome: 'failed',
      failureStage: failureStage,
    );
    setState(() {
      _pairing = false;
      _error = message;
    });
  }

  @override
  Widget build(BuildContext context) {
    final setup = _setup;
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      appBar: AppBar(
        backgroundColor: CelestialColors.backgroundDark,
        foregroundColor: CelestialColors.textPrimary,
        title: const Text('Add Button'),
      ),
      body: SafeArea(
        top: false,
        child: ListView(
          padding: const EdgeInsets.fromLTRB(22, 20, 22, 32),
          children: [
            const Icon(Icons.touch_app_rounded, color: _accent, size: 58),
            const SizedBox(height: 20),
            const Text(
              'Put the button in pairing mode',
              textAlign: TextAlign.center,
              style: TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 24,
                fontWeight: FontWeight.w700,
              ),
            ),
            const SizedBox(height: 12),
            Text(
              'Follow the button’s pairing-mode instructions, then keep it '
              'close to the Rhythm Box. Rhythm will match only the button '
              'identified by the QR code you scanned.',
              textAlign: TextAlign.center,
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.86),
                fontSize: 15,
                height: 1.45,
              ),
            ),
            if (setup != null) ...[
              const SizedBox(height: 20),
              Text(
                'Button ••••${setup.bleIdentity.substring(8)}',
                textAlign: TextAlign.center,
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.64),
                  fontSize: 13,
                ),
              ),
            ],
            if (_error != null) ...[
              const SizedBox(height: 24),
              Container(
                key: const ValueKey('aidot-pairing-error'),
                padding: const EdgeInsets.all(14),
                decoration: BoxDecoration(
                  color: Colors.red.withValues(alpha: 0.1),
                  borderRadius: BorderRadius.circular(12),
                  border: Border.all(color: Colors.red.withValues(alpha: 0.35)),
                ),
                child: Text(
                  _error!,
                  style: const TextStyle(color: CelestialColors.textPrimary),
                ),
              ),
            ],
            const SizedBox(height: 30),
            SizedBox(
              height: 54,
              child: FilledButton.icon(
                key: const ValueKey('find-aidot-button'),
                onPressed: _pairing || setup == null ? null : _startPairing,
                style: FilledButton.styleFrom(backgroundColor: _accent),
                icon: _pairing
                    ? const SizedBox(
                        width: 20,
                        height: 20,
                        child: CircularProgressIndicator(
                          strokeWidth: 2,
                          color: Colors.white,
                        ),
                      )
                    : const Icon(Icons.bluetooth_searching_rounded),
                label: Text(_pairing ? 'Looking for Button…' : 'Find Button'),
              ),
            ),
          ],
        ),
      ),
    );
  }
}
