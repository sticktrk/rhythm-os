import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';
import 'package:rhythm_sdk/rhythm_sdk.dart' show RhythmConnection;

import '../../widgets/solar_orbit.dart';

class MatterDevicePairingResult {
  const MatterDevicePairingResult({
    required this.nativeDeviceId,
    required this.name,
    required this.deviceType,
    this.manufacturer,
    this.model,
  });

  final String nativeDeviceId;
  final String name;
  final String deviceType;
  final String? manufacturer;
  final String? model;
}

enum _PairingPhase { input, pairing, failed }

/// Full-screen modal for pairing a Matter device through Rhythm's backend.
class MatterDeviceAddScreen extends StatefulWidget {
  const MatterDeviceAddScreen({super.key});

  static Future<MatterDevicePairingResult?> show(BuildContext context) {
    return Navigator.of(context).push(
      PageRouteBuilder(
        opaque: false,
        barrierColor: Colors.black54,
        pageBuilder: (context, animation, secondaryAnimation) {
          return const MatterDeviceAddScreen();
        },
        transitionsBuilder: (context, animation, secondaryAnimation, child) {
          final curve = CurvedAnimation(
            parent: animation,
            curve: Curves.easeOutCubic,
            reverseCurve: Curves.easeInCubic,
          );
          return SlideTransition(
            position: Tween<Offset>(
              begin: const Offset(0, 1),
              end: Offset.zero,
            ).animate(curve),
            child: child,
          );
        },
        transitionDuration: const Duration(milliseconds: 350),
        reverseTransitionDuration: const Duration(milliseconds: 300),
      ),
    );
  }

  @override
  State<MatterDeviceAddScreen> createState() => _MatterDeviceAddScreenState();
}

class _MatterDeviceAddScreenState extends State<MatterDeviceAddScreen>
    with SingleTickerProviderStateMixin {
  static const _teal = Color(0xFF00BCD4);
  static const _tealDeep = Color(0xFF00838F);
  static const _segmentLengths = [4, 3, 4];

  final _codeControllers = List.generate(3, (_) => TextEditingController());
  final _codeFocuses = List.generate(3, (_) => FocusNode());
  final _prevLengths = [0, 0, 0];
  late final List<VoidCallback> _segmentListeners;
  late final AnimationController _pulseController;

  _PairingPhase _phase = _PairingPhase.input;
  String? _errorText;

  @override
  void initState() {
    super.initState();
    _pulseController = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 2000),
    )..repeat(reverse: true);

    _segmentListeners = List.generate(3, (i) => () => _onSegmentChanged(i));
    for (int i = 0; i < 3; i++) {
      _codeControllers[i].addListener(_segmentListeners[i]);
    }

    for (int i = 1; i < 3; i++) {
      _codeFocuses[i].onKeyEvent = (node, event) {
        if (event is KeyDownEvent &&
            event.logicalKey == LogicalKeyboardKey.backspace &&
            _codeControllers[i].text.isEmpty) {
          _codeFocuses[i - 1].requestFocus();
          final previous = _codeControllers[i - 1];
          if (previous.text.isNotEmpty) {
            previous.text =
                previous.text.substring(0, previous.text.length - 1);
          }
          return KeyEventResult.handled;
        }
        return KeyEventResult.ignored;
      };
    }
  }

  @override
  void dispose() {
    _pulseController.dispose();
    for (int i = 0; i < 3; i++) {
      _codeControllers[i].removeListener(_segmentListeners[i]);
    }
    for (final controller in _codeControllers) {
      controller.dispose();
    }
    for (final focusNode in _codeFocuses) {
      focusNode.dispose();
    }
    super.dispose();
  }

  void _onSegmentChanged(int segment) {
    final text = _codeControllers[segment].text;
    final previousLength = _prevLengths[segment];
    _prevLengths[segment] = text.length;

    if (text.length > _segmentLengths[segment] &&
        text.length - previousLength > 1) {
      _distributeCode(text);
      return;
    }

    if (text.length > _segmentLengths[segment]) {
      _removeSegmentListeners();
      _codeControllers[segment].text =
          text.substring(0, _segmentLengths[segment]);
      _prevLengths[segment] = _segmentLengths[segment];
      _addSegmentListeners();
      if (segment < 2) _codeFocuses[segment + 1].requestFocus();
      setState(() {});
      return;
    }

    if (text.length == _segmentLengths[segment] &&
        previousLength < _segmentLengths[segment] &&
        segment < 2) {
      _codeFocuses[segment + 1].requestFocus();
    }

    if (text.isEmpty && previousLength > 0 && segment > 0) {
      _codeFocuses[segment - 1].requestFocus();
    }
  }

  void _removeSegmentListeners() {
    for (int i = 0; i < 3; i++) {
      _codeControllers[i].removeListener(_segmentListeners[i]);
    }
  }

  void _addSegmentListeners() {
    for (int i = 0; i < 3; i++) {
      _codeControllers[i].addListener(_segmentListeners[i]);
    }
  }

  void _distributeCode(String rawText) {
    final clean = rawText.replaceAll(RegExp(r'\D'), '');
    _removeSegmentListeners();
    int offset = 0;
    for (int i = 0; i < 3; i++) {
      final end = (offset + _segmentLengths[i]).clamp(0, clean.length);
      _codeControllers[i].text = clean.substring(offset, end);
      _prevLengths[i] = _codeControllers[i].text.length;
      offset = end;
    }
    _addSegmentListeners();

    for (int i = 0; i < 3; i++) {
      if (_codeControllers[i].text.length < _segmentLengths[i]) {
        _codeFocuses[i].requestFocus();
        setState(() {});
        return;
      }
    }

    _codeFocuses[2].requestFocus();
    setState(() {});
  }

  String get _manualCodeDigits =>
      _codeControllers.map((controller) => controller.text).join();

  String get _manualCode =>
      '${_codeControllers[0].text}-${_codeControllers[1].text}-${_codeControllers[2].text}';

  bool get _codeComplete => _manualCodeDigits.length == 11;

  Future<void> _startPairing() async {
    if (!_codeComplete) return;

    FocusScope.of(context).unfocus();
    HapticFeedback.mediumImpact();
    setState(() {
      _phase = _PairingPhase.pairing;
      _errorText = null;
    });

    final connection = context.read<RhythmConnection>();
    final result = await connection.api.pairDevice(
      hubType: 'matter',
      params: {
        'setup_payload': _manualCode,
        'network': 'wifi',
        'rendezvous': 'on_network',
      },
      receiveTimeout: const Duration(seconds: 45),
    );

    if (!mounted) return;

    if (result == null) {
      _showPairingError('Could not reach the server.');
      return;
    }

    final httpStatus = (result['http_status'] as num?)?.toInt();
    final error = result['error'] as String?;
    if (httpStatus != null && httpStatus != 200) {
      _showPairingError(
        'The server rejected the pairing request.',
        detail: error,
      );
      return;
    }

    final status = result['status'] as String?;
    final device = result['device'] as Map<String, dynamic>?;

    if (status == 'failed') {
      _showPairingError('Pairing failed.', detail: error);
      return;
    }

    if (status == 'complete' && device != null) {
      final nativeDeviceId = device['device_id'] as String? ?? '';
      if (nativeDeviceId.isEmpty) {
        _showPairingError(
          'Pairing completed, but the server returned no device ID.',
        );
        return;
      }

      HapticFeedback.heavyImpact();
      Navigator.of(context).pop(
        MatterDevicePairingResult(
          nativeDeviceId: nativeDeviceId,
          name: device['name'] as String? ?? 'Matter Device',
          deviceType: device['device_type'] as String? ?? 'light',
          manufacturer: device['manufacturer'] as String?,
          model: device['model'] as String?,
        ),
      );
      return;
    }

    _showPairingError(
      'Pairing did not complete.',
      detail: error ?? 'Unexpected status: ${status ?? 'unknown'}',
    );
  }

  void _showPairingError(String message, {String? detail}) {
    setState(() {
      _phase = _PairingPhase.failed;
      _errorText =
          detail == null || detail.isEmpty ? message : '$message\n\n$detail';
    });
  }

  void _resetToInput() {
    setState(() {
      _phase = _PairingPhase.input;
      _errorText = null;
    });
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      body: SafeArea(
        child: Column(
          children: [
            _buildHeader(),
            Expanded(
              child: SingleChildScrollView(
                padding: const EdgeInsets.symmetric(horizontal: 24),
                child: switch (_phase) {
                  _PairingPhase.input => _buildInputPhase(),
                  _PairingPhase.pairing => _buildPairingPhase(),
                  _PairingPhase.failed => _buildFailurePhase(),
                },
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildHeader() {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
      child: Row(
        children: [
          GestureDetector(
            onTap: () => Navigator.of(context).pop(),
            child: Container(
              width: 40,
              height: 40,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: _teal.withValues(alpha: 0.15),
                border: Border.all(
                  color: _teal.withValues(alpha: 0.3),
                  width: 1,
                ),
              ),
              child: const Icon(Icons.close, color: _teal, size: 20),
            ),
          ),
          const Expanded(
            child: Text(
              'Add Matter Device',
              textAlign: TextAlign.center,
              style: TextStyle(
                color: CelestialColors.textPrimary,
                fontSize: 18,
                fontWeight: FontWeight.w600,
                letterSpacing: 0.3,
              ),
            ),
          ),
          const SizedBox(width: 40),
        ],
      ),
    );
  }

  Widget _buildInputPhase() {
    return Column(
      children: [
        const SizedBox(height: 40),
        AnimatedBuilder(
          animation: _pulseController,
          builder: (context, child) {
            final glow = 0.1 + _pulseController.value * 0.1;
            return Container(
              width: 96,
              height: 96,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                gradient: RadialGradient(
                  colors: [
                    _teal.withValues(alpha: glow),
                    Colors.transparent,
                  ],
                  radius: 1.5,
                ),
              ),
              child: Center(
                child: Container(
                  width: 64,
                  height: 64,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    gradient: LinearGradient(
                      begin: Alignment.topLeft,
                      end: Alignment.bottomRight,
                      colors: [_teal, _tealDeep],
                    ),
                  ),
                  child: const Icon(
                    Icons.memory_outlined,
                    color: Colors.white,
                    size: 28,
                  ),
                ),
              ),
            );
          },
        ),
        const SizedBox(height: 32),
        Text(
          'Enter Manual Pairing Code',
          style: TextStyle(
            color: CelestialColors.textPrimary,
            fontSize: 20,
            fontWeight: FontWeight.w600,
          ),
        ),
        const SizedBox(height: 8),
        Text(
          'Use the 11-digit code printed on your Matter device.',
          textAlign: TextAlign.center,
          style: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.7),
            fontSize: 14,
          ),
        ),
        const SizedBox(height: 12),
        Text(
          'Rhythm handles the local Matter hub setup automatically. Pairing usually takes 15–30 seconds.',
          textAlign: TextAlign.center,
          style: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.55),
            fontSize: 13,
            height: 1.4,
          ),
        ),
        const SizedBox(height: 40),
        Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            _buildCodeSegment(0, 4),
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 8),
              child: Text(
                '–',
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.4),
                  fontSize: 24,
                  fontWeight: FontWeight.w300,
                ),
              ),
            ),
            _buildCodeSegment(1, 3),
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 8),
              child: Text(
                '–',
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.4),
                  fontSize: 24,
                  fontWeight: FontWeight.w300,
                ),
              ),
            ),
            _buildCodeSegment(2, 4),
          ],
        ),
        const SizedBox(height: 48),
        SizedBox(
          width: double.infinity,
          height: 52,
          child: DecoratedBox(
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(14),
              gradient: _codeComplete
                  ? const LinearGradient(colors: [_teal, _tealDeep])
                  : null,
              color: _codeComplete
                  ? null
                  : CelestialColors.textSecondary.withValues(alpha: 0.15),
            ),
            child: MaterialButton(
              onPressed: _codeComplete ? _startPairing : null,
              shape: RoundedRectangleBorder(
                borderRadius: BorderRadius.circular(14),
              ),
              child: Text(
                'Pair Device',
                style: TextStyle(
                  color: _codeComplete
                      ? Colors.white
                      : CelestialColors.textSecondary.withValues(alpha: 0.4),
                  fontSize: 16,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
          ),
        ),
        const SizedBox(height: 20),
        _buildInfoCard(
          title: 'Right now',
          lines: const [
            'Only Matter-over-Wi-Fi with on-network rendezvous is supported.',
            'If you only have a QR code, enter the printed manual pairing code instead. Raw Matter QR payloads are not accepted yet.',
          ],
        ),
        const SizedBox(height: 40),
      ],
    );
  }

  Widget _buildCodeSegment(int index, int maxLength) {
    return SizedBox(
      width: maxLength == 4 ? 80 : 64,
      child: TextField(
        controller: _codeControllers[index],
        focusNode: _codeFocuses[index],
        keyboardType: TextInputType.number,
        textAlign: TextAlign.center,
        style: const TextStyle(
          color: CelestialColors.textPrimary,
          fontSize: 16,
          fontWeight: FontWeight.w600,
          letterSpacing: 2,
        ),
        inputFormatters: [FilteringTextInputFormatter.digitsOnly],
        decoration: InputDecoration(
          counterText: '',
          contentPadding:
              const EdgeInsets.symmetric(horizontal: 8, vertical: 14),
          filled: true,
          fillColor: CelestialColors.backgroundCard,
          enabledBorder: OutlineInputBorder(
            borderRadius: BorderRadius.circular(12),
            borderSide: BorderSide(
              color: CelestialColors.orbitRing.withValues(alpha: 0.5),
            ),
          ),
          focusedBorder: OutlineInputBorder(
            borderRadius: BorderRadius.circular(12),
            borderSide: const BorderSide(color: _teal, width: 1.5),
          ),
        ),
        onChanged: (_) => setState(() {}),
      ),
    );
  }

  Widget _buildPairingPhase() {
    return Column(
      children: [
        const SizedBox(height: 60),
        AnimatedBuilder(
          animation: _pulseController,
          builder: (context, child) {
            final pulse = _pulseController.value;
            return SizedBox(
              width: 140,
              height: 140,
              child: Stack(
                alignment: Alignment.center,
                children: [
                  Container(
                    width: 140,
                    height: 140,
                    decoration: BoxDecoration(
                      shape: BoxShape.circle,
                      border: Border.all(
                        color: _teal.withValues(alpha: 0.1 + pulse * 0.15),
                        width: 1,
                      ),
                    ),
                  ),
                  Container(
                    width: 100,
                    height: 100,
                    decoration: BoxDecoration(
                      shape: BoxShape.circle,
                      border: Border.all(
                        color: _teal.withValues(alpha: 0.15 + pulse * 0.2),
                        width: 1.5,
                      ),
                    ),
                  ),
                  Container(
                    width: 64,
                    height: 64,
                    decoration: BoxDecoration(
                      shape: BoxShape.circle,
                      gradient: LinearGradient(
                        begin: Alignment.topLeft,
                        end: Alignment.bottomRight,
                        colors: [_teal, _tealDeep],
                      ),
                      boxShadow: [
                        BoxShadow(
                          color: _teal.withValues(alpha: 0.3 + pulse * 0.2),
                          blurRadius: 20 + pulse * 10,
                          spreadRadius: pulse * 4,
                        ),
                      ],
                    ),
                    child: const Icon(
                      Icons.memory_outlined,
                      color: Colors.white,
                      size: 28,
                    ),
                  ),
                ],
              ),
            );
          },
        ),
        const SizedBox(height: 32),
        Text(
          'Pairing Device...',
          style: TextStyle(
            color: CelestialColors.textPrimary,
            fontSize: 20,
            fontWeight: FontWeight.w600,
          ),
        ),
        const SizedBox(height: 8),
        Text(
          'Keep this screen open while Rhythm scans your network over mDNS and commissions the device.',
          textAlign: TextAlign.center,
          style: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.7),
            fontSize: 14,
            height: 1.4,
          ),
        ),
        const SizedBox(height: 24),
        _buildInfoCard(
          title: 'This request is synchronous',
          lines: const [
            'Pairing usually completes in about 15–30 seconds.',
            'No separate Matter hub connection step is needed in the app.',
          ],
        ),
        const SizedBox(height: 32),
        SizedBox(
          width: 20,
          height: 20,
          child: CircularProgressIndicator(
            strokeWidth: 2,
            valueColor: AlwaysStoppedAnimation(_teal.withValues(alpha: 0.6)),
          ),
        ),
        const SizedBox(height: 40),
      ],
    );
  }

  Widget _buildFailurePhase() {
    return Column(
      children: [
        const SizedBox(height: 60),
        Container(
          width: 72,
          height: 72,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: Colors.red.shade900.withValues(alpha: 0.22),
            border: Border.all(
              color: Colors.red.shade300.withValues(alpha: 0.35),
            ),
          ),
          child: Icon(Icons.close, color: Colors.red.shade300, size: 28),
        ),
        const SizedBox(height: 28),
        Text(
          'Pairing Failed',
          style: TextStyle(
            color: Colors.red.shade300,
            fontSize: 20,
            fontWeight: FontWeight.w600,
          ),
        ),
        const SizedBox(height: 10),
        Text(
          _errorText ?? 'Pairing failed.',
          textAlign: TextAlign.center,
          style: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.75),
            fontSize: 14,
            height: 1.45,
          ),
        ),
        const SizedBox(height: 32),
        SizedBox(
          width: double.infinity,
          height: 52,
          child: DecoratedBox(
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(14),
              gradient: const LinearGradient(colors: [_teal, _tealDeep]),
            ),
            child: MaterialButton(
              onPressed: _resetToInput,
              shape: RoundedRectangleBorder(
                borderRadius: BorderRadius.circular(14),
              ),
              child: const Text(
                'Try Again',
                style: TextStyle(
                  color: Colors.white,
                  fontSize: 16,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
          ),
        ),
        const SizedBox(height: 12),
        SizedBox(
          width: double.infinity,
          height: 52,
          child: OutlinedButton(
            onPressed: () => Navigator.of(context).pop(),
            style: OutlinedButton.styleFrom(
              side: BorderSide(color: _teal.withValues(alpha: 0.35)),
              shape: RoundedRectangleBorder(
                borderRadius: BorderRadius.circular(14),
              ),
            ),
            child: const Text(
              'Close',
              style: TextStyle(
                color: _teal,
                fontSize: 16,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
        ),
        const SizedBox(height: 40),
      ],
    );
  }

  Widget _buildInfoCard({
    required String title,
    required List<String> lines,
  }) {
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        color: CelestialColors.backgroundCard,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(
          color: CelestialColors.orbitRing.withValues(alpha: 0.35),
        ),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            title,
            style: TextStyle(
              color: CelestialColors.textPrimary,
              fontSize: 13,
              fontWeight: FontWeight.w600,
            ),
          ),
          const SizedBox(height: 8),
          for (final line in lines) ...[
            Text(
              line,
              style: TextStyle(
                color: CelestialColors.textSecondary.withValues(alpha: 0.7),
                fontSize: 13,
                height: 1.4,
              ),
            ),
            if (line != lines.last) const SizedBox(height: 6),
          ],
        ],
      ),
    );
  }
}
