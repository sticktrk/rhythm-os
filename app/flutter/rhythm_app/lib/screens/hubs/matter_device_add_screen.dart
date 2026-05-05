import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:rhythm_core/rhythm_core.dart' show HubEndpoint;
import 'package:rhythm_sdk/rhythm_sdk.dart';

import '../../services/matter_setup_payload.dart';
import '../../widgets/solar_orbit.dart';
import 'matter_add_method.dart';
import 'matter_qr_scanner_screen.dart';

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
  const MatterDeviceAddScreen({
    super.key,
    required this.endpoint,
    required this.addMethod,
  });

  final HubEndpoint endpoint;
  final MatterAddMethod addMethod;

  static Future<MatterDevicePairingResult?> show(
    BuildContext context, {
    required HubEndpoint endpoint,
    required MatterAddMethod addMethod,
  }) {
    return Navigator.of(context).push(
      PageRouteBuilder(
        opaque: false,
        barrierColor: Colors.black54,
        pageBuilder: (context, animation, secondaryAnimation) {
          return MatterDeviceAddScreen(
            endpoint: endpoint,
            addMethod: addMethod,
          );
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

  final _setupPayloadController = TextEditingController();
  late final AnimationController _pulseController;
  late final RhythmMatterApi _pairingApi;

  _PairingPhase _phase = _PairingPhase.input;
  String? _errorText;

  @override
  void initState() {
    super.initState();
    _pairingApi = RhythmMatterApi(baseUrl: widget.endpoint.baseUrl);
    _pulseController = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 2000),
    )..repeat(reverse: true);
    _setupPayloadController.addListener(_handlePayloadChanged);
  }

  @override
  void dispose() {
    _setupPayloadController.removeListener(_handlePayloadChanged);
    _setupPayloadController.dispose();
    _pulseController.dispose();
    super.dispose();
  }

  bool get _supportsQrScan {
    if (kIsWeb) return false;
    return defaultTargetPlatform == TargetPlatform.android ||
        defaultTargetPlatform == TargetPlatform.iOS ||
        defaultTargetPlatform == TargetPlatform.macOS;
  }

  String get _setupPayload =>
      normalizeMatterSetupPayload(_setupPayloadController.text);

  bool get _hasSetupPayload => _setupPayload.isNotEmpty;

  bool get _looksLikeMatterPayload =>
      isLikelyMatterSetupPayload(_setupPayloadController.text);

  bool get _usesWifiCommissioningPreflight =>
      widget.addMethod.requiresWifiCommissioningPreflight;

  void _handlePayloadChanged() {
    if (mounted) {
      setState(() {});
    }
  }

  Future<void> _scanQrCode() async {
    final payload = await MatterQrScannerScreen.show(context);
    if (!mounted || payload == null) return;

    _setupPayloadController.text = payload;
    _setupPayloadController.selection = TextSelection.collapsed(
      offset: payload.length,
    );

    await _startPairing();
  }

  Future<void> _startPairing() async {
    final setupPayload = _setupPayload;
    if (setupPayload.isEmpty) return;

    FocusScope.of(context).unfocus();
    HapticFeedback.mediumImpact();
    setState(() {
      _phase = _PairingPhase.pairing;
      _errorText = null;
    });

    if (_usesWifiCommissioningPreflight) {
      final wifiStatus = await _pairingApi.getWifiStatus();
      if (!mounted) return;

      if (wifiStatus != null && !wifiStatus.readyForMatterPairing) {
        final details = <String>[
          if (wifiStatus.configPresent != true)
            'Wi-Fi configuration is not present on the server appliance.',
          if (wifiStatus.connected != true)
            'The server appliance is not currently connected to Wi-Fi.',
          'Connect the appliance to Wi-Fi first, then try again.',
          'You do not need to enter network credentials here.',
        ];
        _showPairingError(
          'This Rhythm appliance is not ready to add a new device.',
          detail: details.join(' '),
        );
        return;
      }
    }

    final result = await _pairingApi.pairDevice(
      setupPayload: setupPayload,
      rendezvous: 'auto',
      network: 'wifi',
      receiveTimeout: const Duration(seconds: 45),
    );

    if (!mounted) return;

    if (result.httpStatus != null && result.httpStatus != 200) {
      _showPairingError(
        'The server rejected the pairing request.',
        detail: result.error,
      );
      return;
    }

    if (result.status == 'failed') {
      _showPairingError('Pairing failed.', detail: result.error);
      return;
    }

    final device = result.device;
    if (result.status == 'complete' && device != null) {
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
          name: device['name'] as String? ?? 'Device',
          deviceType: device['device_type'] as String? ?? 'light',
          manufacturer: device['manufacturer'] as String?,
          model: device['model'] as String?,
        ),
      );
      return;
    }

    _showPairingError(
      'Pairing did not complete.',
      detail:
          result.error ?? 'Unexpected status: ${result.status ?? 'unknown'}',
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
          Expanded(
            child: Text(
              widget.addMethod.actionLabel,
              textAlign: TextAlign.center,
              style: const TextStyle(
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
          switch (widget.addMethod) {
            MatterAddMethod.automatic => 'Add Device',
            MatterAddMethod.onNetworkSetupCode => 'Add Device',
            MatterAddMethod.bleWifiCommissioning => 'Add Device',
          },
          style: TextStyle(
            color: CelestialColors.textPrimary,
            fontSize: 20,
            fontWeight: FontWeight.w600,
          ),
        ),
        const SizedBox(height: 8),
        Text(
          switch (widget.addMethod) {
            MatterAddMethod.automatic =>
              'Scan the QR code, paste the setup payload, or enter the manual code from the device.',
            MatterAddMethod.onNetworkSetupCode =>
              'Enter the setup code or scan the QR payload from the device.',
            MatterAddMethod.bleWifiCommissioning =>
              'Power on the device nearby, then enter its setup code or QR payload to continue.',
          },
          textAlign: TextAlign.center,
          style: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.7),
            fontSize: 14,
          ),
        ),
        const SizedBox(height: 12),
        Text(
          switch (widget.addMethod) {
            MatterAddMethod.automatic =>
              'Rhythm sends the setup data to the server and the server chooses the best available path.',
            MatterAddMethod.onNetworkSetupCode =>
              'Rhythm sends the setup data to the server and asks it to add the device.',
            MatterAddMethod.bleWifiCommissioning =>
              'Rhythm sends the setup data to the server and asks it to finish setup for the device.',
          },
          textAlign: TextAlign.center,
          style: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.55),
            fontSize: 13,
            height: 1.4,
          ),
        ),
        const SizedBox(height: 32),
        if (_supportsQrScan) ...[
          SizedBox(
            width: double.infinity,
            height: 52,
            child: OutlinedButton.icon(
              onPressed: _scanQrCode,
              style: OutlinedButton.styleFrom(
                side: BorderSide(color: _teal.withValues(alpha: 0.45)),
                backgroundColor: _teal.withValues(alpha: 0.06),
                shape: RoundedRectangleBorder(
                  borderRadius: BorderRadius.circular(14),
                ),
              ),
              icon: const Icon(
                Icons.qr_code_scanner_rounded,
                color: _teal,
              ),
              label: const Text(
                'Scan QR Code',
                style: TextStyle(
                  color: _teal,
                  fontSize: 16,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
          ),
          const SizedBox(height: 20),
        ],
        TextField(
          controller: _setupPayloadController,
          keyboardType: TextInputType.visiblePassword,
          textInputAction: TextInputAction.done,
          autocorrect: false,
          enableSuggestions: false,
          style: const TextStyle(
            color: CelestialColors.textPrimary,
            fontSize: 15,
            fontWeight: FontWeight.w500,
          ),
          minLines: 1,
          maxLines: 3,
          decoration: InputDecoration(
            labelText: 'Setup payload or manual code',
            hintText: 'MT:Y.K908OC16750648G00 or 3497-123-4567',
            helperText: _setupPayloadController.text.isEmpty
                ? 'Paste the setup payload or the code printed on the device.'
                : (_looksLikeMatterPayload
                    ? 'Ready to send to the server.'
                    : 'This does not look like a typical setup code, but you can still try adding the device.'),
            contentPadding: const EdgeInsets.symmetric(
              horizontal: 16,
              vertical: 16,
            ),
            filled: true,
            fillColor: CelestialColors.backgroundCard,
            enabledBorder: OutlineInputBorder(
              borderRadius: BorderRadius.circular(14),
              borderSide: BorderSide(
                color: CelestialColors.orbitRing.withValues(alpha: 0.5),
              ),
            ),
            focusedBorder: OutlineInputBorder(
              borderRadius: BorderRadius.circular(14),
              borderSide: const BorderSide(color: _teal, width: 1.5),
            ),
            labelStyle: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.8),
            ),
            hintStyle: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.45),
            ),
            helperStyle: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.65),
            ),
          ),
        ),
        const SizedBox(height: 32),
        SizedBox(
          width: double.infinity,
          height: 52,
          child: DecoratedBox(
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(14),
              gradient: _hasSetupPayload
                  ? const LinearGradient(colors: [_teal, _tealDeep])
                  : null,
              color: _hasSetupPayload
                  ? null
                  : CelestialColors.textSecondary.withValues(alpha: 0.15),
            ),
            child: MaterialButton(
              onPressed: _hasSetupPayload ? _startPairing : null,
              shape: RoundedRectangleBorder(
                borderRadius: BorderRadius.circular(14),
              ),
              child: Text(
                switch (widget.addMethod) {
                  MatterAddMethod.automatic => 'Add Device',
                  MatterAddMethod.onNetworkSetupCode => 'Add Device',
                  MatterAddMethod.bleWifiCommissioning => 'Add Device',
                },
                style: TextStyle(
                  color: _hasSetupPayload
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
          title: 'What Happens Next',
          lines: switch (widget.addMethod) {
            MatterAddMethod.automatic => const [
                'Rhythm sends the setup data to the server.',
                'The server picks the best available way to add the device.',
                'You do not need a separate hub setup flow here.',
              ],
            MatterAddMethod.onNetworkSetupCode => const [
                'Use this when the device is already ready to be added.',
                'Rhythm sends the setup data to the server.',
                'You do not need a separate hub setup flow here.',
              ],
            MatterAddMethod.bleWifiCommissioning => const [
                'Use this when the device still needs a full setup path.',
                'The server appliance must already be connected and ready.',
                'You do not need to enter network credentials here.',
              ],
          },
        ),
        const SizedBox(height: 40),
      ],
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
          switch (widget.addMethod) {
            MatterAddMethod.automatic => 'Adding Device...',
            MatterAddMethod.onNetworkSetupCode => 'Adding Device...',
            MatterAddMethod.bleWifiCommissioning => 'Adding Device...',
          },
          style: TextStyle(
            color: CelestialColors.textPrimary,
            fontSize: 20,
            fontWeight: FontWeight.w600,
          ),
        ),
        const SizedBox(height: 8),
        Text(
          switch (widget.addMethod) {
            MatterAddMethod.automatic =>
              'Keep this screen open while Rhythm adds the device.',
            MatterAddMethod.onNetworkSetupCode =>
              'Keep this screen open while Rhythm finds the device and adds it.',
            MatterAddMethod.bleWifiCommissioning =>
              'Keep this screen open while Rhythm completes setup and adds the device.',
          },
          textAlign: TextAlign.center,
          style: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.7),
            fontSize: 14,
            height: 1.4,
          ),
        ),
        const SizedBox(height: 16),
        Text(
          _setupPayload,
          textAlign: TextAlign.center,
          style: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.48),
            fontSize: 12,
            height: 1.35,
          ),
        ),
        const SizedBox(height: 24),
        _buildInfoCard(
          title: 'This request is synchronous',
          lines: [
            'Adding usually completes in about 15-30 seconds.',
            'No separate hub connection step is needed in the app.',
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
