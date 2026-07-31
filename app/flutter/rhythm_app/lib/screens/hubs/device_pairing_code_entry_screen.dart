import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../services/analytics_service.dart';
import '../../services/device_pairing_code.dart';
import '../../widgets/solar_orbit.dart';
import 'device_pairing_scanner_screen.dart';

class DevicePairingCodeEntryScreen extends StatefulWidget {
  const DevicePairingCodeEntryScreen({
    super.key,
    this.hueBridgeSerialSearchAvailable = false,
    this.aidotButtonPairingAvailable = false,
    this.hueBridgeOnly = false,
  });

  final bool hueBridgeSerialSearchAvailable;
  final bool aidotButtonPairingAvailable;
  final bool hueBridgeOnly;

  static Future<DevicePairingScannerResult?> show(
    BuildContext context, {
    bool hueBridgeSerialSearchAvailable = false,
    bool aidotButtonPairingAvailable = false,
    bool hueBridgeOnly = false,
  }) {
    AnalyticsService().logScreenView('device_pairing_code_entry');
    return Navigator.of(context).push<DevicePairingScannerResult>(
      MaterialPageRoute(
        builder: (_) => DevicePairingCodeEntryScreen(
          hueBridgeSerialSearchAvailable: hueBridgeSerialSearchAvailable,
          aidotButtonPairingAvailable: aidotButtonPairingAvailable,
          hueBridgeOnly: hueBridgeOnly,
        ),
      ),
    );
  }

  @override
  State<DevicePairingCodeEntryScreen> createState() =>
      _DevicePairingCodeEntryScreenState();
}

class _DevicePairingCodeEntryScreenState
    extends State<DevicePairingCodeEntryScreen> {
  static const _accent = Color(0xFF26A69A);
  static const _accentBright = Color(0xFF4DD0C8);

  final _controller = TextEditingController();
  DevicePairingGuidance? _guidance;
  DevicePairingCodeDecision? _pendingDecision;

  bool get _hasInput => _controller.text.trim().isNotEmpty;

  @override
  void initState() {
    super.initState();
    _controller.addListener(_handleChanged);
  }

  @override
  void dispose() {
    _controller
      ..removeListener(_handleChanged)
      ..dispose();
    super.dispose();
  }

  void _handleChanged() {
    setState(() {
      _guidance = null;
      _pendingDecision = null;
    });
  }

  Future<void> _pasteCode() async {
    final data = await Clipboard.getData(Clipboard.kTextPlain);
    final text = data?.text?.trim();
    if (text == null || text.isEmpty || !mounted) return;
    _controller
      ..text = text
      ..selection = TextSelection.collapsed(offset: text.length);
    HapticFeedback.selectionClick();
  }

  void _identifyCode() {
    final decision = processDevicePairingCodes(
      [_controller.text],
      hueBridgeSerialSearchAvailable: widget.hueBridgeSerialSearchAvailable,
      aidotButtonPairingAvailable: widget.aidotButtonPairingAvailable,
      hueBridgeOnly: widget.hueBridgeOnly,
    );
    if (decision == null) return;

    final kind = _pairingCodeAnalyticsKind(decision.code.kind);
    if (decision.requiresChoice) {
      AnalyticsService().logDevicePairingCodeDetected(
        codeKind: decision.choices.length > 1 ? 'multiple' : kind,
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
      _continueWithCode(decision.code);
      return;
    }

    AnalyticsService().logDevicePairingCodeDetected(
      codeKind: kind,
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
        DevicePairingCodeKind.matter => DevicePairingScannerResult.matter(
            code.payload,
            inputMethod: 'manual_code',
          ),
        DevicePairingCodeKind.hue => DevicePairingScannerResult.hueBridge(
            normalizeHueBridgeSerial(code.payload)!,
            inputMethod: 'manual_code',
          ),
        DevicePairingCodeKind.aidotButton =>
          DevicePairingScannerResult.aidotButton(
            code.payload,
            inputMethod: 'manual_code',
          ),
        _ => throw StateError('Unsupported pairing-code continuation'),
      },
    );
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: CelestialColors.backgroundDark,
      appBar: AppBar(
        backgroundColor: CelestialColors.backgroundDark,
        foregroundColor: CelestialColors.textPrimary,
        title: Text(
          widget.hueBridgeOnly ? 'Enter Bulb Serial' : 'Enter a Code',
        ),
        elevation: 0,
      ),
      body: SafeArea(
        top: false,
        child: SingleChildScrollView(
          padding: const EdgeInsets.fromLTRB(20, 12, 20, 28),
          keyboardDismissBehavior: ScrollViewKeyboardDismissBehavior.onDrag,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Align(
                alignment: Alignment.centerLeft,
                child: Container(
                  width: 66,
                  height: 66,
                  alignment: Alignment.center,
                  decoration: BoxDecoration(
                    color: _accent.withValues(alpha: 0.16),
                    borderRadius: BorderRadius.circular(20),
                    border: Border.all(
                      color: _accentBright.withValues(alpha: 0.42),
                    ),
                  ),
                  child: const Icon(
                    Icons.keyboard_alt_outlined,
                    color: _accentBright,
                    size: 32,
                  ),
                ),
              ),
              const SizedBox(height: 20),
              Text(
                widget.hueBridgeOnly
                    ? 'Enter Hue bulb serial'
                    : 'Enter any setup code',
                style: TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 26,
                  height: 1.12,
                  fontWeight: FontWeight.w700,
                  letterSpacing: -0.5,
                ),
              ),
              const SizedBox(height: 8),
              Text(
                widget.hueBridgeOnly
                    ? 'Type or paste the six-character serial printed on the '
                        'Hue bulb. Your connected Hue Bridge will search for it.'
                    : 'Type or paste the code printed on your device. Rhythm '
                        'will identify what it is and continue when it can.',
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.82),
                  fontSize: 15,
                  height: 1.45,
                ),
              ),
              const SizedBox(height: 28),
              Container(
                decoration: BoxDecoration(
                  color: CelestialColors.backgroundCard,
                  borderRadius: BorderRadius.circular(16),
                  border: Border.all(
                    color: _guidance == null
                        ? CelestialColors.orbitRing.withValues(alpha: 0.7)
                        : CelestialColors.sunWarm.withValues(alpha: 0.55),
                  ),
                ),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    Padding(
                      padding: const EdgeInsets.fromLTRB(16, 14, 10, 8),
                      child: Row(
                        children: [
                          Expanded(
                            child: Text(
                              widget.hueBridgeOnly
                                  ? 'HUE BULB SERIAL'
                                  : 'DEVICE SETUP CODE',
                              style: TextStyle(
                                color: _accentBright,
                                fontSize: 11,
                                fontWeight: FontWeight.w700,
                                letterSpacing: 1.4,
                              ),
                            ),
                          ),
                          TextButton.icon(
                            onPressed: _pasteCode,
                            icon: const Icon(Icons.content_paste_rounded,
                                size: 16),
                            label: const Text('Paste'),
                          ),
                        ],
                      ),
                    ),
                    Padding(
                      padding: const EdgeInsets.fromLTRB(16, 0, 16, 16),
                      child: TextField(
                        key: const ValueKey('device-pairing-code-input'),
                        controller: _controller,
                        autofocus: true,
                        keyboardType: TextInputType.visiblePassword,
                        textCapitalization: TextCapitalization.characters,
                        textInputAction: TextInputAction.done,
                        autocorrect: false,
                        enableSuggestions: false,
                        minLines: 2,
                        maxLines: 4,
                        onSubmitted: (_) => _hasInput ? _identifyCode() : null,
                        style: const TextStyle(
                          color: CelestialColors.textPrimary,
                          fontSize: 18,
                          height: 1.35,
                          fontFamily: 'monospace',
                          fontWeight: FontWeight.w600,
                          letterSpacing: 0.5,
                        ),
                        decoration: InputDecoration(
                          hintText: widget.hueBridgeOnly
                              ? 'E277DA'
                              : 'Enter the code exactly as shown',
                          hintStyle: TextStyle(
                            color: CelestialColors.textSecondary
                                .withValues(alpha: 0.42),
                            fontSize: 16,
                            fontFamily: null,
                            fontWeight: FontWeight.w400,
                            letterSpacing: 0,
                          ),
                          border: InputBorder.none,
                          isCollapsed: true,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
              const SizedBox(height: 10),
              Text(
                'Spaces and dashes are okay.',
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.58),
                  fontSize: 12,
                ),
              ),
              if (_pendingDecision case final decision?) ...[
                const SizedBox(height: 18),
                _CodeChoiceCard(
                  decision: decision,
                  onSelected: _continueWithCode,
                ),
              ] else if (_guidance case final guidance?) ...[
                const SizedBox(height: 18),
                _CodeGuidanceCard(guidance: guidance),
              ],
              const SizedBox(height: 24),
              SizedBox(
                height: 56,
                child: FilledButton.icon(
                  onPressed: _hasInput ? _identifyCode : null,
                  style: FilledButton.styleFrom(
                    backgroundColor: _accent,
                    foregroundColor: Colors.white,
                    disabledBackgroundColor:
                        CelestialColors.backgroundCard.withValues(alpha: 0.72),
                    disabledForegroundColor:
                        CelestialColors.textSecondary.withValues(alpha: 0.38),
                    shape: RoundedRectangleBorder(
                      borderRadius: BorderRadius.circular(14),
                    ),
                  ),
                  icon: const Icon(Icons.search_rounded),
                  label: Text(
                    widget.hueBridgeOnly
                        ? 'Search with Hue Bridge'
                        : 'Identify & Continue',
                    style: TextStyle(
                      fontSize: 16,
                      fontWeight: FontWeight.w700,
                    ),
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _CodeChoiceCard extends StatelessWidget {
  const _CodeChoiceCard({
    required this.decision,
    required this.onSelected,
  });

  final DevicePairingCodeDecision decision;
  final ValueChanged<DevicePairingCode> onSelected;

  @override
  Widget build(BuildContext context) {
    return Container(
      key: const ValueKey('manual-device-pairing-code-choice'),
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        color:
            _DevicePairingCodeEntryScreenState._accent.withValues(alpha: 0.09),
        borderRadius: BorderRadius.circular(14),
        border: Border.all(
          color: _DevicePairingCodeEntryScreenState._accentBright
              .withValues(alpha: 0.32),
        ),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            'Choose how to add this light',
            style: const TextStyle(
              color: CelestialColors.textPrimary,
              fontSize: 15,
              fontWeight: FontWeight.w700,
            ),
          ),
          const SizedBox(height: 6),
          Text(
            'Choose the connection Rhythm should use.',
            style: TextStyle(
              color: CelestialColors.textSecondary.withValues(alpha: 0.88),
              fontSize: 13,
              height: 1.4,
            ),
          ),
          const SizedBox(height: 12),
          for (final code in decision.choices) ...[
            FilledButton.icon(
              key: ValueKey(
                code.kind == DevicePairingCodeKind.hue
                    ? 'confirm-manual-hue-bridge-serial'
                    : 'confirm-manual-matter-code',
              ),
              onPressed: () => onSelected(code),
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
          ],
        ],
      ),
    );
  }
}

class _CodeGuidanceCard extends StatelessWidget {
  const _CodeGuidanceCard({required this.guidance});

  final DevicePairingGuidance guidance;

  @override
  Widget build(BuildContext context) {
    return Container(
      key: ValueKey(guidance.title),
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        color: CelestialColors.sunWarm.withValues(alpha: 0.09),
        borderRadius: BorderRadius.circular(14),
        border: Border.all(
          color: CelestialColors.sunWarm.withValues(alpha: 0.32),
        ),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Icon(
            Icons.info_outline_rounded,
            color: CelestialColors.sunWarm,
            size: 21,
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  guidance.title,
                  style: const TextStyle(
                    color: CelestialColors.textPrimary,
                    fontSize: 15,
                    fontWeight: FontWeight.w700,
                  ),
                ),
                const SizedBox(height: 5),
                Text(
                  guidance.message,
                  style: TextStyle(
                    color:
                        CelestialColors.textSecondary.withValues(alpha: 0.88),
                    fontSize: 13,
                    height: 1.4,
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

String _pairingCodeAnalyticsKind(DevicePairingCodeKind kind) => switch (kind) {
      DevicePairingCodeKind.matter => 'matter',
      DevicePairingCodeKind.homeKit => 'homekit',
      DevicePairingCodeKind.hue => 'hue',
      DevicePairingCodeKind.aidotButton => 'aidot_button',
      DevicePairingCodeKind.unknown => 'unknown',
    };
