import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:mobile_scanner/mobile_scanner.dart';

import '../../services/matter_setup_payload.dart';
import '../../widgets/solar_orbit.dart';

class MatterQrScannerScreen extends StatefulWidget {
  const MatterQrScannerScreen({super.key});

  static Future<String?> show(BuildContext context) {
    return Navigator.of(context).push<String>(
      PageRouteBuilder<String>(
        opaque: false,
        barrierColor: Colors.black,
        pageBuilder: (context, animation, secondaryAnimation) {
          return const MatterQrScannerScreen();
        },
        transitionsBuilder: (context, animation, secondaryAnimation, child) {
          final curve = CurvedAnimation(
            parent: animation,
            curve: Curves.easeOutCubic,
            reverseCurve: Curves.easeInCubic,
          );
          return FadeTransition(
            opacity: curve,
            child: child,
          );
        },
      ),
    );
  }

  @override
  State<MatterQrScannerScreen> createState() => _MatterQrScannerScreenState();
}

class _MatterQrScannerScreenState extends State<MatterQrScannerScreen> {
  static const _teal = Color(0xFF00BCD4);

  final MobileScannerController _controller = MobileScannerController();
  bool _handledDetection = false;

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  void _handleDetect(BarcodeCapture capture) {
    if (_handledDetection) return;

    final rawValue = capture.barcodes
        .map((barcode) => normalizeMatterSetupPayload(barcode.rawValue ?? ''))
        .firstWhere(
          (value) => value.isNotEmpty,
          orElse: () => '',
        );

    if (rawValue.isEmpty) return;

    if (!isLikelyMatterSetupPayload(rawValue)) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(
          content: Text('That QR code does not look like a Matter setup code.'),
        ),
      );
      return;
    }

    _handledDetection = true;
    HapticFeedback.mediumImpact();
    Navigator.of(context).pop(rawValue);
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: Colors.black,
      body: Stack(
        fit: StackFit.expand,
        children: [
          MobileScanner(
            controller: _controller,
            onDetect: _handleDetect,
          ),
          IgnorePointer(
            child: DecoratedBox(
              decoration: BoxDecoration(
                gradient: LinearGradient(
                  begin: Alignment.topCenter,
                  end: Alignment.bottomCenter,
                  colors: [
                    Colors.black.withValues(alpha: 0.72),
                    Colors.transparent,
                    Colors.black.withValues(alpha: 0.72),
                  ],
                  stops: const [0.0, 0.28, 1.0],
                ),
              ),
            ),
          ),
          SafeArea(
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 16),
              child: Column(
                children: [
                  Row(
                    children: [
                      GestureDetector(
                        onTap: () => Navigator.of(context).pop(),
                        child: Container(
                          width: 40,
                          height: 40,
                          decoration: BoxDecoration(
                            shape: BoxShape.circle,
                            color: Colors.black.withValues(alpha: 0.45),
                            border: Border.all(
                              color: Colors.white.withValues(alpha: 0.14),
                            ),
                          ),
                          child: const Icon(
                            Icons.close,
                            color: Colors.white,
                            size: 20,
                          ),
                        ),
                      ),
                      const Expanded(
                        child: Text(
                          'Scan Matter QR Code',
                          textAlign: TextAlign.center,
                          style: TextStyle(
                            color: Colors.white,
                            fontSize: 18,
                            fontWeight: FontWeight.w600,
                            letterSpacing: 0.3,
                          ),
                        ),
                      ),
                      const SizedBox(width: 40),
                    ],
                  ),
                  const Spacer(),
                  Container(
                    width: 260,
                    height: 260,
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
                        Positioned(
                          top: 18,
                          left: 18,
                          child: _buildCorner(Alignment.topLeft),
                        ),
                        Positioned(
                          top: 18,
                          right: 18,
                          child: _buildCorner(Alignment.topRight),
                        ),
                        Positioned(
                          bottom: 18,
                          left: 18,
                          child: _buildCorner(Alignment.bottomLeft),
                        ),
                        Positioned(
                          bottom: 18,
                          right: 18,
                          child: _buildCorner(Alignment.bottomRight),
                        ),
                      ],
                    ),
                  ),
                  const SizedBox(height: 24),
                  Text(
                    'Align the Matter QR code inside the frame.',
                    textAlign: TextAlign.center,
                    style: TextStyle(
                      color: Colors.white.withValues(alpha: 0.92),
                      fontSize: 16,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                  const SizedBox(height: 8),
                  Text(
                    'Rhythm will send the raw setup payload to the server for BLE or on-network commissioning.',
                    textAlign: TextAlign.center,
                    style: TextStyle(
                      color: Colors.white.withValues(alpha: 0.72),
                      fontSize: 13,
                      height: 1.4,
                    ),
                  ),
                  const SizedBox(height: 28),
                  Container(
                    width: double.infinity,
                    padding: const EdgeInsets.all(16),
                    decoration: BoxDecoration(
                      color: CelestialColors.backgroundCard.withValues(
                        alpha: 0.9,
                      ),
                      borderRadius: BorderRadius.circular(16),
                      border: Border.all(
                        color: CelestialColors.orbitRing.withValues(alpha: 0.5),
                      ),
                    ),
                    child: Text(
                      'If scanning fails, close this view and paste the raw MT: payload or manual code instead.',
                      textAlign: TextAlign.center,
                      style: TextStyle(
                        color: CelestialColors.textSecondary.withValues(
                          alpha: 0.88,
                        ),
                        fontSize: 13,
                        height: 1.4,
                      ),
                    ),
                  ),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildCorner(Alignment alignment) {
    final isTop = alignment.y < 0;
    final isLeft = alignment.x < 0;

    return SizedBox(
      width: 34,
      height: 34,
      child: CustomPaint(
        painter: _CornerPainter(
          color: _teal,
          isTop: isTop,
          isLeft: isLeft,
        ),
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
  bool shouldRepaint(_CornerPainter oldDelegate) {
    return color != oldDelegate.color ||
        isTop != oldDelegate.isTop ||
        isLeft != oldDelegate.isLeft;
  }
}
