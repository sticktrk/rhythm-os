import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:mobile_scanner/mobile_scanner.dart';

import '../../services/analytics_service.dart';
import '../../services/device_pairing_code.dart';
import '../../widgets/solar_orbit.dart';

enum DevicePairingScannerAction { matter, hueBridge, aidotButton, enterCode }

class DevicePairingScannerResult {
  const DevicePairingScannerResult._({
    required this.action,
    this.payload,
    this.inputMethod = 'camera',
  });

  const DevicePairingScannerResult.matter(
    String payload, {
    String inputMethod = 'camera',
  }) : this._(
          action: DevicePairingScannerAction.matter,
          payload: payload,
          inputMethod: inputMethod,
        );

  const DevicePairingScannerResult.hueBridge(
    String serial, {
    String inputMethod = 'camera',
  }) : this._(
          action: DevicePairingScannerAction.hueBridge,
          payload: serial,
          inputMethod: inputMethod,
        );

  const DevicePairingScannerResult.aidotButton(
    String payload, {
    String inputMethod = 'camera',
  }) : this._(
          action: DevicePairingScannerAction.aidotButton,
          payload: payload,
          inputMethod: inputMethod,
        );

  const DevicePairingScannerResult.enterCode()
      : this._(action: DevicePairingScannerAction.enterCode);

  final DevicePairingScannerAction action;
  final String? payload;
  final String inputMethod;
}

typedef DevicePairingCameraBuilder = Widget Function(
  BuildContext context,
  ValueChanged<Iterable<String>> onDetect,
);

bool get supportsDevicePairingCamera {
  if (kIsWeb) return false;
  return defaultTargetPlatform == TargetPlatform.android ||
      defaultTargetPlatform == TargetPlatform.iOS ||
      defaultTargetPlatform == TargetPlatform.macOS;
}

String _pairingCodeAnalyticsKind(DevicePairingCodeKind kind) => switch (kind) {
      DevicePairingCodeKind.matter => 'matter',
      DevicePairingCodeKind.homeKit => 'homekit',
      DevicePairingCodeKind.hue => 'hue',
      DevicePairingCodeKind.aidotButton => 'aidot_button',
      DevicePairingCodeKind.unknown => 'unknown',
    };

class DevicePairingScannerScreen extends StatefulWidget {
  const DevicePairingScannerScreen({
    super.key,
    this.showEnterCodeAction = true,
    this.hueBridgeSerialSearchAvailable = false,
    this.aidotButtonPairingAvailable = false,
    this.hueBridgeOnly = false,
    @visibleForTesting this.cameraBuilder,
  });

  final bool showEnterCodeAction;
  final bool hueBridgeSerialSearchAvailable;
  final bool aidotButtonPairingAvailable;
  final bool hueBridgeOnly;
  final DevicePairingCameraBuilder? cameraBuilder;

  static Future<DevicePairingScannerResult?> show(
    BuildContext context, {
    bool showEnterCodeAction = true,
    bool hueBridgeSerialSearchAvailable = false,
    bool aidotButtonPairingAvailable = false,
    bool hueBridgeOnly = false,
  }) {
    return Navigator.of(context).push<DevicePairingScannerResult>(
      PageRouteBuilder<DevicePairingScannerResult>(
        opaque: false,
        barrierColor: Colors.black,
        pageBuilder: (context, animation, secondaryAnimation) {
          return DevicePairingScannerScreen(
            showEnterCodeAction: showEnterCodeAction,
            hueBridgeSerialSearchAvailable: hueBridgeSerialSearchAvailable,
            aidotButtonPairingAvailable: aidotButtonPairingAvailable,
            hueBridgeOnly: hueBridgeOnly,
          );
        },
        transitionsBuilder: (context, animation, secondaryAnimation, child) {
          final curve = CurvedAnimation(
            parent: animation,
            curve: Curves.easeOutCubic,
            reverseCurve: Curves.easeInCubic,
          );
          return FadeTransition(opacity: curve, child: child);
        },
      ),
    );
  }

  @override
  State<DevicePairingScannerScreen> createState() =>
      _DevicePairingScannerScreenState();
}

class _DevicePairingScannerScreenState
    extends State<DevicePairingScannerScreen> {
  static const _teal = Color(0xFF00BCD4);

  MobileScannerController? _controller;
  DevicePairingGuidance? _guidance;
  DevicePairingCodeDecision? _pendingDecision;
  bool _handledDetection = false;

  @override
  void initState() {
    super.initState();
    AnalyticsService().logScreenView('device_pairing_scanner');
    if (widget.cameraBuilder == null) {
      _controller = MobileScannerController(
        formats: const [BarcodeFormat.qrCode],
      );
    }
  }

  @override
  void dispose() {
    _controller?.dispose();
    super.dispose();
  }

  void _handleCapture(BarcodeCapture capture) {
    _handleRawValues(capture.barcodes.map((barcode) => barcode.rawValue ?? ''));
  }

  @visibleForTesting
  void handleRawValues(Iterable<String> values) {
    _handleRawValues(values);
  }

  void _handleRawValues(Iterable<String> values) {
    if (_handledDetection) return;

    final decision = processDevicePairingCodes(
      values,
      hueBridgeSerialSearchAvailable: widget.hueBridgeSerialSearchAvailable,
      aidotButtonPairingAvailable: widget.aidotButtonPairingAvailable,
      hueBridgeOnly: widget.hueBridgeOnly,
    );
    if (decision == null) return;
    final code = decision.code;

    _handledDetection = true;
    if (decision.requiresChoice) {
      AnalyticsService().logDevicePairingCodeDetected(
        codeKind: decision.choices.length > 1
            ? 'multiple'
            : _pairingCodeAnalyticsKind(code.kind),
        outcome:
            decision.choices.length > 1 ? 'choice_shown' : 'confirmation_shown',
      );
      HapticFeedback.lightImpact();
      setState(() {
        _guidance = null;
        _pendingDecision = decision;
      });
      return;
    }

    if (decision.canContinue) {
      _continueWithCode(code);
      return;
    }

    AnalyticsService().logDevicePairingCodeDetected(
      codeKind: _pairingCodeAnalyticsKind(code.kind),
      outcome: 'guidance_shown',
    );
    HapticFeedback.lightImpact();
    setState(() {
      _pendingDecision = null;
      _guidance = decision.guidance;
    });
  }

  void _continueWithCode(DevicePairingCode code) {
    AnalyticsService().logDevicePairingCodeDetected(
      codeKind: _pairingCodeAnalyticsKind(code.kind),
      outcome: 'continued_to_pairing',
    );
    HapticFeedback.mediumImpact();
    Navigator.of(context).pop(
      switch (code.kind) {
        DevicePairingCodeKind.matter =>
          DevicePairingScannerResult.matter(code.payload),
        DevicePairingCodeKind.hue => DevicePairingScannerResult.hueBridge(
            normalizeHueBridgeSerial(code.payload)!,
          ),
        DevicePairingCodeKind.aidotButton =>
          DevicePairingScannerResult.aidotButton(code.payload),
        _ => throw StateError('Unsupported pairing-code continuation'),
      },
    );
  }

  void _scanAgain() {
    HapticFeedback.selectionClick();
    setState(() {
      _guidance = null;
      _pendingDecision = null;
      _handledDetection = false;
    });
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: Colors.black,
      body: Stack(
        fit: StackFit.expand,
        children: [
          if (widget.cameraBuilder case final cameraBuilder?)
            cameraBuilder(context, _handleRawValues)
          else
            MobileScanner(controller: _controller, onDetect: _handleCapture),
          IgnorePointer(
            child: DecoratedBox(
              decoration: BoxDecoration(
                gradient: LinearGradient(
                  begin: Alignment.topCenter,
                  end: Alignment.bottomCenter,
                  colors: [
                    Colors.black.withValues(alpha: 0.78),
                    Colors.transparent,
                    Colors.black.withValues(alpha: 0.82),
                  ],
                  stops: const [0.0, 0.32, 1.0],
                ),
              ),
            ),
          ),
          SafeArea(
            child: LayoutBuilder(
              builder: (context, constraints) {
                final compact = constraints.maxHeight < 700;
                final compactGuidance =
                    compact && (_guidance != null || _pendingDecision != null);
                return Padding(
                  padding: EdgeInsets.fromLTRB(
                    20,
                    compact ? 8 : 16,
                    20,
                    compact ? 12 : 20,
                  ),
                  child: Column(
                    children: [
                      _buildHeader(context),
                      const Spacer(),
                      _buildViewfinder(
                        compactGuidance ? 130 : (compact ? 210 : 264),
                      ),
                      SizedBox(height: compact ? 14 : 24),
                      Text(
                        widget.hueBridgeOnly
                            ? 'Scan Hue bulb QR'
                            : 'Scan any device QR code',
                        textAlign: TextAlign.center,
                        style: TextStyle(
                          color: Colors.white,
                          fontSize: compact ? 17 : 19,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                      if (!compactGuidance) ...[
                        SizedBox(height: compact ? 4 : 8),
                        Text(
                          widget.hueBridgeOnly
                              ? 'Scan the QR beside the six-character serial '
                                  'printed on the bulb.'
                              : 'Rhythm will identify the code and continue '
                                  'when it can.',
                          textAlign: TextAlign.center,
                          style: TextStyle(
                            color: Colors.white.withValues(alpha: 0.72),
                            fontSize: compact ? 12 : 13,
                            height: 1.4,
                          ),
                        ),
                      ],
                      SizedBox(
                        height: compactGuidance ? 8 : (compact ? 14 : 24),
                      ),
                      AnimatedSwitcher(
                        duration: const Duration(milliseconds: 220),
                        child: _pendingDecision != null
                            ? _buildPairingChoice(_pendingDecision!)
                            : _guidance != null
                                ? _buildGuidance(_guidance!)
                                : _buildScannerActions(),
                      ),
                    ],
                  ),
                );
              },
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildHeader(BuildContext context) {
    return Row(
      children: [
        IconButton(
          tooltip: widget.hueBridgeOnly
              ? 'Back to Hue Bridge'
              : 'Back to Add & Review',
          onPressed: () => Navigator.of(context).pop(),
          style: IconButton.styleFrom(
            backgroundColor: Colors.black.withValues(alpha: 0.45),
            foregroundColor: Colors.white,
            side: BorderSide(color: Colors.white.withValues(alpha: 0.14)),
          ),
          icon: const Icon(Icons.arrow_back_rounded),
        ),
        Expanded(
          child: Text(
            widget.hueBridgeOnly ? 'Add Hue Bulb' : 'Add Device',
            textAlign: TextAlign.center,
            style: TextStyle(
              color: Colors.white,
              fontSize: 18,
              fontWeight: FontWeight.w600,
              letterSpacing: 0.3,
            ),
          ),
        ),
        const SizedBox(width: 48),
      ],
    );
  }

  Widget _buildViewfinder(double size) {
    return Semantics(
      label: 'QR code scan area',
      child: Container(
        width: size,
        height: size,
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(28),
          border: Border.all(color: _teal, width: 2),
          boxShadow: [
            BoxShadow(
              color: _teal.withValues(alpha: 0.24),
              blurRadius: 30,
              spreadRadius: 6,
            ),
          ],
        ),
        child: Stack(
          children: [
            _corner(top: 18, left: 18, alignment: Alignment.topLeft),
            _corner(top: 18, right: 18, alignment: Alignment.topRight),
            _corner(bottom: 18, left: 18, alignment: Alignment.bottomLeft),
            _corner(bottom: 18, right: 18, alignment: Alignment.bottomRight),
          ],
        ),
      ),
    );
  }

  Widget _corner({
    double? top,
    double? right,
    double? bottom,
    double? left,
    required Alignment alignment,
  }) {
    return Positioned(
      top: top,
      right: right,
      bottom: bottom,
      left: left,
      child: SizedBox(
        width: 34,
        height: 34,
        child: CustomPaint(
          painter: _CornerPainter(
            color: _teal,
            isTop: alignment.y < 0,
            isLeft: alignment.x < 0,
          ),
        ),
      ),
    );
  }

  Widget _buildScannerActions() {
    return Container(
      key: const ValueKey('scanner-actions'),
      width: double.infinity,
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard.withValues(alpha: 0.92),
        borderRadius: BorderRadius.circular(18),
        border: Border.all(
          color: CelestialColors.orbitRing.withValues(alpha: 0.55),
        ),
      ),
      child: Column(
        children: [
          Text(
            widget.hueBridgeOnly
                ? 'Hold the bulb QR steady inside the frame.'
                : 'Hold the code steady inside the frame.',
            textAlign: TextAlign.center,
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.88),
              fontSize: 13,
              height: 1.4,
            ),
          ),
          if (widget.showEnterCodeAction) ...[
            const SizedBox(height: 14),
            SizedBox(
              width: double.infinity,
              child: FilledButton.tonalIcon(
                onPressed: () {
                  AnalyticsService().logDevicePairingCodeDetected(
                    codeKind: 'manual',
                    outcome: 'manual_entry_selected',
                  );
                  Navigator.of(context).pop(
                    const DevicePairingScannerResult.enterCode(),
                  );
                },
                icon: const Icon(Icons.keyboard_rounded),
                label: Text(
                  widget.hueBridgeOnly ? 'Enter Bulb Serial' : 'Enter a Code',
                ),
              ),
            ),
          ],
        ],
      ),
    );
  }

  Widget _buildGuidance(DevicePairingGuidance guidance) {
    return Container(
      key: ValueKey(guidance.title),
      width: double.infinity,
      padding: const EdgeInsets.fromLTRB(18, 16, 18, 16),
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard.withValues(alpha: 0.96),
        borderRadius: BorderRadius.circular(18),
        border: Border.all(color: _teal.withValues(alpha: 0.4)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            guidance.title,
            style: const TextStyle(
              color: CelestialColors.textPrimary,
              fontSize: 16,
              fontWeight: FontWeight.w700,
            ),
          ),
          const SizedBox(height: 6),
          Text(
            guidance.message,
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.9),
              fontSize: 13,
              height: 1.4,
            ),
          ),
          const SizedBox(height: 14),
          OutlinedButton.icon(
            onPressed: _scanAgain,
            icon: const Icon(Icons.qr_code_scanner_rounded),
            label: Text(
              widget.hueBridgeOnly
                  ? 'Scan Another Bulb QR'
                  : 'Scan Another Code',
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildPairingChoice(DevicePairingCodeDecision decision) {
    final hasMultipleChoices = decision.choices.length > 1;
    return Container(
      key: const ValueKey('device-pairing-code-choice'),
      width: double.infinity,
      padding: const EdgeInsets.fromLTRB(18, 16, 18, 16),
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard.withValues(alpha: 0.96),
        borderRadius: BorderRadius.circular(18),
        border: Border.all(color: _teal.withValues(alpha: 0.4)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            hasMultipleChoices ? 'Choose how to add this light' : '',
            style: const TextStyle(
              color: CelestialColors.textPrimary,
              fontSize: 16,
              fontWeight: FontWeight.w700,
            ),
          ),
          const SizedBox(height: 6),
          Text(
            hasMultipleChoices
                ? 'This scan found both a Matter setup code and a Hue bulb '
                    'serial. Choose the connection Rhythm should use.'
                : '',
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.9),
              fontSize: 13,
              height: 1.4,
            ),
          ),
          const SizedBox(height: 14),
          for (final code in decision.choices) ...[
            FilledButton.icon(
              key: ValueKey(
                code.kind == DevicePairingCodeKind.hue
                    ? 'choose-hue-bridge'
                    : 'choose-matter',
              ),
              onPressed: () => _continueWithCode(code),
              icon: Icon(
                code.kind == DevicePairingCodeKind.hue
                    ? Icons.hub_rounded
                    : Icons.hub_rounded,
              ),
              label: Text(
                code.kind == DevicePairingCodeKind.hue
                    ? 'Search with Hue Bridge'
                    : 'Pair with Matter',
              ),
            ),
            const SizedBox(height: 8),
          ],
          OutlinedButton.icon(
            onPressed: _scanAgain,
            icon: const Icon(Icons.qr_code_scanner_rounded),
            label: const Text('Scan Another Code'),
          ),
        ],
      ),
    );
  }
}

class _CornerPainter extends CustomPainter {
  const _CornerPainter({
    required this.color,
    required this.isTop,
    required this.isLeft,
  });

  final Color color;
  final bool isTop;
  final bool isLeft;

  @override
  void paint(Canvas canvas, Size size) {
    final paint = Paint()
      ..color = color
      ..strokeWidth = 4
      ..strokeCap = StrokeCap.round
      ..style = PaintingStyle.stroke;

    final horizontalY = isTop ? 0.0 : size.height;
    final verticalX = isLeft ? 0.0 : size.width;

    canvas.drawLine(
      Offset(isLeft ? 0.0 : size.width - 16, horizontalY),
      Offset(isLeft ? 16 : size.width, horizontalY),
      paint,
    );
    canvas.drawLine(
      Offset(verticalX, isTop ? 0.0 : size.height - 16),
      Offset(verticalX, isTop ? 16 : size.height),
      paint,
    );
  }

  @override
  bool shouldRepaint(covariant _CornerPainter oldDelegate) {
    return oldDelegate.color != color ||
        oldDelegate.isTop != isTop ||
        oldDelegate.isLeft != isLeft;
  }
}
