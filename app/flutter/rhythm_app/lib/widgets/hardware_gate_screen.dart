import 'dart:math' as math;
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:url_launcher/url_launcher.dart';

import '../services/analytics_service.dart';
import '../services/virtual_experience_service.dart';
import 'connect_hub_screen.dart';
import 'solar_orbit.dart';

/// "Do you have hardware yet?" funnel for users who land in the app without a
/// LightBox or RhythmOS server.
///
/// Step 1 — `_GateStep.gate`     The Yes/No question.
/// Step 2 — `_GateStep.upsell`   Sales-style explainer for users without one.
/// Step 3 — `_GateStep.connect`  The existing [ConnectHubScreen].
///
/// Replaces the direct `ConnectHubScreen` previously rendered by
/// `_buildNoRoomsLayout` in `app_shell.dart`.
class HardwareOnboardingGate extends StatefulWidget {
  final ConnectHubMode mode;

  /// Optional account (Sign in / Log out) control, anchored into the gate's
  /// own header so it sits inside this screen's composition instead of
  /// floating over it from the app shell. Shown only on the gate/upsell
  /// steps — the connect step has its own header and sign-in affordance.
  final Widget? accountControl;

  const HardwareOnboardingGate({
    super.key,
    this.mode = ConnectHubMode.rhythmServer,
    this.accountControl,
  });

  @override
  State<HardwareOnboardingGate> createState() => _HardwareOnboardingGateState();
}

enum _GateStep { gate, upsell, connect }

class _HardwareOnboardingGateState extends State<HardwareOnboardingGate> {
  _GateStep _step = _GateStep.gate;

  void _toConnect() {
    HapticFeedback.mediumImpact();
    AnalyticsService().logEvent('onboarding_hardware_choice', {
      'choice': 'has_hardware',
    });
    AnalyticsService().logScreenView('connect_hub');
    setState(() => _step = _GateStep.connect);
  }

  void _toUpsell() {
    HapticFeedback.selectionClick();
    AnalyticsService().logEvent('onboarding_hardware_choice', {
      'choice': 'no_hardware',
    });
    setState(() => _step = _GateStep.upsell);
  }

  void _backToGate() {
    HapticFeedback.selectionClick();
    setState(() => _step = _GateStep.gate);
  }

  @override
  Widget build(BuildContext context) {
    return Stack(
      fit: StackFit.expand,
      children: [
        Positioned.fill(
          child: TickerMode(
            enabled: _step == _GateStep.connect,
            child: Offstage(
              offstage: _step != _GateStep.connect,
              child: ConnectHubScreen(
                mode: widget.mode,
                trackScreenView: false,
              ),
            ),
          ),
        ),
        if (_step != _GateStep.connect)
          Positioned.fill(
            child: AnimatedSwitcher(
              duration: const Duration(milliseconds: 380),
              switchInCurve: Curves.easeOutCubic,
              switchOutCurve: Curves.easeInCubic,
              transitionBuilder: (child, anim) {
                return FadeTransition(
                  opacity: anim,
                  child: SlideTransition(
                    position: Tween<Offset>(
                      begin: const Offset(0, 0.04),
                      end: Offset.zero,
                    ).animate(anim),
                    child: child,
                  ),
                );
              },
              child: switch (_step) {
                _GateStep.gate => _HardwareGateScreen(
                    key: const ValueKey('gate'),
                    onYes: _toConnect,
                    onNo: _toUpsell,
                  ),
                _GateStep.upsell => _GetLightBoxScreen(
                    key: const ValueKey('upsell'),
                    onBack: _backToGate,
                    onIHaveOne: _toConnect,
                  ),
                _GateStep.connect => const SizedBox.shrink(),
              },
            ),
          ),
        if (_step == _GateStep.connect)
          Positioned(
            top: 8,
            left: 8,
            child: _BackChevron(onTap: _backToGate),
          ),
        // Account control lives inside the gate's own SafeArea header so it
        // can never overlap a sub-screen's title. Hidden on the connect step,
        // which carries its own header and "Sign in for saved Homes" entry.
        if (widget.accountControl != null && _step != _GateStep.connect)
          Positioned(
            top: 0,
            right: 0,
            child: SafeArea(
              child: Padding(
                padding: const EdgeInsets.only(top: 4, right: 10),
                child: widget.accountControl,
              ),
            ),
          ),
      ],
    );
  }
}

class _BackChevron extends StatelessWidget {
  final VoidCallback onTap;
  const _BackChevron({required this.onTap});

  @override
  Widget build(BuildContext context) {
    return Material(
      color: Colors.transparent,
      child: InkWell(
        onTap: () {
          HapticFeedback.selectionClick();
          onTap();
        },
        borderRadius: BorderRadius.circular(22),
        child: Container(
          width: 44,
          height: 44,
          alignment: Alignment.center,
          child: Icon(
            Icons.arrow_back_rounded,
            color: CelestialColors.textSecondary.withValues(alpha: 0.85),
            size: 22,
          ),
        ),
      ),
    );
  }
}

// ═══════════════════════════════════════════════════════════════════════════
// Hardware gate question
// ═══════════════════════════════════════════════════════════════════════════

class _HardwareGateScreen extends StatefulWidget {
  final VoidCallback onYes;
  final VoidCallback onNo;

  const _HardwareGateScreen(
      {super.key, required this.onYes, required this.onNo});

  @override
  State<_HardwareGateScreen> createState() => _HardwareGateScreenState();
}

class _HardwareGateScreenState extends State<_HardwareGateScreen>
    with TickerProviderStateMixin {
  late final AnimationController _reveal;
  late final AnimationController _twinkle;
  late final AnimationController _pulse;

  @override
  void initState() {
    super.initState();
    AnalyticsService().logScreenView('onboarding_hardware_gate');

    _reveal = AnimationController(
      duration: const Duration(milliseconds: 1400),
      vsync: this,
    );
    WidgetsBinding.instance.addPostFrameCallback((_) => _reveal.forward());

    _twinkle = AnimationController(
      duration: const Duration(seconds: 9),
      vsync: this,
    )..repeat();

    _pulse = AnimationController(
      duration: const Duration(milliseconds: 2400),
      vsync: this,
    )..repeat(reverse: true);
  }

  @override
  void dispose() {
    _reveal.dispose();
    _twinkle.dispose();
    _pulse.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return ColoredBox(
      color: CelestialColors.backgroundDark,
      child: Stack(
        fit: StackFit.expand,
        children: [
          const _AuroraGlow(),
          AnimatedBuilder(
            animation: _twinkle,
            builder: (context, _) =>
                CustomPaint(painter: _StarfieldPainter(t: _twinkle.value)),
          ),
          const _HorizonGlow(),
          SafeArea(
            child: Padding(
              padding: const EdgeInsets.fromLTRB(28, 20, 28, 28),
              child: LayoutBuilder(
                builder: (context, constraints) {
                  return SingleChildScrollView(
                    child: ConstrainedBox(
                      constraints:
                          BoxConstraints(minHeight: constraints.maxHeight),
                      child: IntrinsicHeight(
                        child: Column(
                          children: [
                            _Reveal(
                              controller: _reveal,
                              start: 0.0,
                              child: const _Eyebrow(
                                text: 'GET STARTED',
                              ),
                            ),
                            const Spacer(flex: 4),
                            _Reveal(
                              controller: _reveal,
                              start: 0.10,
                              child: AnimatedBuilder(
                                animation: _pulse,
                                builder: (context, _) =>
                                    _LightBoxBeaconMini(beat: _pulse.value),
                              ),
                            ),
                            const SizedBox(height: 36),
                            _Reveal(
                              controller: _reveal,
                              start: 0.22,
                              child: const _GateHeadline(),
                            ),
                            const SizedBox(height: 14),
                            _Reveal(
                              controller: _reveal,
                              start: 0.34,
                              child: const _GateSub(),
                            ),
                            const Spacer(flex: 5),
                            _Reveal(
                              controller: _reveal,
                              start: 0.50,
                              child: _YesAnswerCard(onTap: widget.onYes),
                            ),
                            const SizedBox(height: 12),
                            _Reveal(
                              controller: _reveal,
                              start: 0.62,
                              child: _NoAnswerCard(onTap: widget.onNo),
                            ),
                            const SizedBox(height: 16),
                            _Reveal(
                              controller: _reveal,
                              start: 0.70,
                              child: _VirtualExperienceLink(
                                onTap: () {
                                  HapticFeedback.selectionClick();
                                  AnalyticsService().logEvent(
                                    'onboarding_hardware_choice',
                                    {'choice': 'virtual_experience'},
                                  );
                                  VirtualExperienceService.instance.enter();
                                },
                              ),
                            ),
                            const Spacer(flex: 1),
                            _Reveal(
                              controller: _reveal,
                              start: 0.82,
                              child: _Footnote(),
                            ),
                            const SizedBox(height: 8),
                          ],
                        ),
                      ),
                    ),
                  );
                },
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _GateHeadline extends StatelessWidget {
  const _GateHeadline();

  @override
  Widget build(BuildContext context) {
    return ShaderMask(
      shaderCallback: (rect) => ui.Gradient.linear(
        Offset(rect.left, 0),
        Offset(rect.right, 0),
        const [
          CelestialColors.sunWarm,
          Color(0xFFFFE0B2),
          Color(0xFF8AB4F8),
        ],
        const [0.0, 0.55, 1.0],
      ),
      blendMode: BlendMode.srcIn,
      child: const Text(
        'Do you have\na LightBox?',
        textAlign: TextAlign.center,
        style: TextStyle(
          color: Colors.white,
          fontSize: 38,
          fontWeight: FontWeight.w300,
          letterSpacing: -0.6,
          height: 1.06,
        ),
      ),
    );
  }
}

class _GateSub extends StatelessWidget {
  const _GateSub();

  @override
  Widget build(BuildContext context) {
    return Text(
      'Or anything else running RhythmOS.',
      textAlign: TextAlign.center,
      style: TextStyle(
        color: CelestialColors.textSecondary.withValues(alpha: 0.78),
        fontSize: 15,
        fontWeight: FontWeight.w400,
        letterSpacing: 0.2,
        height: 1.4,
      ),
    );
  }
}

class _YesAnswerCard extends StatefulWidget {
  final VoidCallback onTap;
  const _YesAnswerCard({required this.onTap});

  @override
  State<_YesAnswerCard> createState() => _YesAnswerCardState();
}

class _YesAnswerCardState extends State<_YesAnswerCard>
    with SingleTickerProviderStateMixin {
  late final AnimationController _glow;
  bool _pressed = false;

  @override
  void initState() {
    super.initState();
    _glow = AnimationController(
      duration: const Duration(milliseconds: 2600),
      vsync: this,
    )..repeat(reverse: true);
  }

  @override
  void dispose() {
    _glow.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: _glow,
      builder: (context, _) {
        final b = _glow.value;
        return GestureDetector(
          onTapDown: (_) => setState(() => _pressed = true),
          onTapUp: (_) => setState(() => _pressed = false),
          onTapCancel: () => setState(() => _pressed = false),
          onTap: widget.onTap,
          child: AnimatedScale(
            duration: const Duration(milliseconds: 110),
            scale: _pressed ? 0.98 : 1.0,
            child: Container(
              padding: const EdgeInsets.symmetric(horizontal: 22, vertical: 18),
              decoration: BoxDecoration(
                borderRadius: BorderRadius.circular(18),
                gradient: LinearGradient(
                  begin: Alignment.topLeft,
                  end: Alignment.bottomRight,
                  colors: [
                    CelestialColors.sunWarm.withValues(alpha: 0.92),
                    const Color(0xFFE8821C).withValues(alpha: 0.88),
                  ],
                ),
                boxShadow: [
                  BoxShadow(
                    color: CelestialColors.sunWarm
                        .withValues(alpha: 0.30 + 0.20 * b),
                    blurRadius: 28 + 8 * b,
                    spreadRadius: -2,
                    offset: const Offset(0, 8),
                  ),
                  BoxShadow(
                    color: const Color(0xFFE8821C).withValues(alpha: 0.18),
                    blurRadius: 50,
                    spreadRadius: -8,
                    offset: const Offset(0, 18),
                  ),
                ],
              ),
              child: Row(
                children: [
                  // Glyph: a tiny solid sun
                  Container(
                    width: 38,
                    height: 38,
                    decoration: BoxDecoration(
                      shape: BoxShape.circle,
                      gradient: const RadialGradient(
                        colors: [
                          Colors.white,
                          Color(0xFFFFE6B0),
                          Color(0xFFFFB54A),
                        ],
                        stops: [0.0, 0.45, 1.0],
                      ),
                      boxShadow: [
                        BoxShadow(
                          color: Colors.white.withValues(alpha: 0.55 * b),
                          blurRadius: 14,
                        ),
                      ],
                    ),
                  ),
                  const SizedBox(width: 14),
                  const Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        Text(
                          'I have one',
                          style: TextStyle(
                            color: Color(0xFF1A1206),
                            fontSize: 17,
                            fontWeight: FontWeight.w700,
                            letterSpacing: 0.1,
                          ),
                        ),
                        SizedBox(height: 3),
                        Text(
                          "Let's pair it to your home",
                          style: TextStyle(
                            color: Color(0xCC1A1206),
                            fontSize: 12.5,
                            fontWeight: FontWeight.w500,
                            letterSpacing: 0.2,
                          ),
                        ),
                      ],
                    ),
                  ),
                  const Icon(
                    Icons.arrow_forward_rounded,
                    color: Color(0xFF1A1206),
                    size: 22,
                  ),
                ],
              ),
            ),
          ),
        );
      },
    );
  }
}

class _NoAnswerCard extends StatefulWidget {
  final VoidCallback onTap;
  const _NoAnswerCard({required this.onTap});

  @override
  State<_NoAnswerCard> createState() => _NoAnswerCardState();
}

class _NoAnswerCardState extends State<_NoAnswerCard> {
  bool _pressed = false;

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTapDown: (_) => setState(() => _pressed = true),
      onTapUp: (_) => setState(() => _pressed = false),
      onTapCancel: () => setState(() => _pressed = false),
      onTap: widget.onTap,
      child: AnimatedScale(
        duration: const Duration(milliseconds: 110),
        scale: _pressed ? 0.98 : 1.0,
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 22, vertical: 18),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(18),
            color: Colors.white.withValues(alpha: 0.02),
            border: Border.all(
              color: Colors.white.withValues(alpha: 0.14),
              width: 1,
            ),
          ),
          child: Row(
            children: [
              SizedBox(
                width: 38,
                height: 38,
                child: CustomPaint(painter: _CrescentPainter()),
              ),
              const SizedBox(width: 14),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(
                      'Not yet',
                      style: TextStyle(
                        color: CelestialColors.textPrimary,
                        fontSize: 17,
                        fontWeight: FontWeight.w600,
                        letterSpacing: 0.1,
                      ),
                    ),
                    const SizedBox(height: 3),
                    Text(
                      "Let's get started",
                      style: TextStyle(
                        color: CelestialColors.textSecondary
                            .withValues(alpha: 0.72),
                        fontSize: 12.5,
                        fontWeight: FontWeight.w500,
                        letterSpacing: 0.2,
                      ),
                    ),
                  ],
                ),
              ),
              Icon(
                Icons.arrow_forward_rounded,
                color: CelestialColors.textSecondary.withValues(alpha: 0.55),
                size: 22,
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// Subtle third option on the gate — drops the user into a fully populated
/// demo with no sign-in or hardware. Sits between the two answer cards and
/// the footnote so it reads as "explore" rather than another primary path.
class _VirtualExperienceLink extends StatelessWidget {
  final VoidCallback onTap;
  const _VirtualExperienceLink({required this.onTap});

  @override
  Widget build(BuildContext context) {
    return Center(
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          onTap: onTap,
          borderRadius: BorderRadius.circular(12),
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Container(
                  width: 6,
                  height: 6,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: CelestialColors.sunWarm,
                    boxShadow: [
                      BoxShadow(
                        color: CelestialColors.sunWarm.withValues(alpha: 0.55),
                        blurRadius: 6,
                        spreadRadius: 0.5,
                      ),
                    ],
                  ),
                ),
                const SizedBox(width: 10),
                Text(
                  'Try a Virtual Experience',
                  style: TextStyle(
                    color: CelestialColors.textPrimary.withValues(alpha: 0.88),
                    fontSize: 14,
                    fontWeight: FontWeight.w500,
                    letterSpacing: 0.4,
                  ),
                ),
                const SizedBox(width: 6),
                Icon(
                  Icons.arrow_forward_rounded,
                  size: 16,
                  color: CelestialColors.textPrimary.withValues(alpha: 0.7),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _Footnote extends StatelessWidget {
  @override
  Widget build(BuildContext context) {
    return Text(
      'A LightBox or any device running RhythmOS\nis the brain that drives your lights.',
      textAlign: TextAlign.center,
      style: TextStyle(
        color: CelestialColors.textSecondary.withValues(alpha: 0.4),
        fontSize: 11,
        fontWeight: FontWeight.w400,
        letterSpacing: 0.3,
        height: 1.55,
      ),
    );
  }
}

class _CrescentPainter extends CustomPainter {
  @override
  void paint(Canvas canvas, Size size) {
    final c = Offset(size.width / 2, size.height / 2);
    final r = size.width * 0.38;

    // Outer ring
    final ringPaint = Paint()
      ..color = const Color(0xFF8AB4F8).withValues(alpha: 0.45)
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.2;
    canvas.drawCircle(c, r, ringPaint);

    // Crescent shadow — clipping a circle by another offset circle.
    canvas.save();
    canvas.clipPath(Path()..addOval(Rect.fromCircle(center: c, radius: r)));
    final shadow = Paint()
      ..color = CelestialColors.backgroundDark.withValues(alpha: 0.92)
      ..style = PaintingStyle.fill;
    canvas.drawCircle(c.translate(r * 0.5, -r * 0.15), r, shadow);
    canvas.restore();

    // Inner glow on the lit edge
    final glowPaint = Paint()
      ..shader = ui.Gradient.radial(
        c.translate(-r * 0.35, r * 0.05),
        r * 0.9,
        [
          const Color(0xFFAFCBFF).withValues(alpha: 0.50),
          const Color(0xFFAFCBFF).withValues(alpha: 0.0),
        ],
      );
    canvas.drawCircle(c, r * 0.95, glowPaint);
  }

  @override
  bool shouldRepaint(covariant _CrescentPainter oldDelegate) => false;
}

/// A small standalone "LightBox" illustration used at the top of the gate.
/// Reuses the same volumetric beacon idea as the upsell hero, scaled down.
class _LightBoxBeaconMini extends StatelessWidget {
  final double beat; // 0..1
  const _LightBoxBeaconMini({required this.beat});

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      width: 220,
      height: 180,
      child:
          CustomPaint(painter: _LightBoxBeaconPainter(beat: beat, mini: true)),
    );
  }
}

// ═══════════════════════════════════════════════════════════════════════════
// Step 2 — "Get a LightBox" upsell
// ═══════════════════════════════════════════════════════════════════════════

class _GetLightBoxScreen extends StatefulWidget {
  final VoidCallback onBack;
  final VoidCallback onIHaveOne;

  const _GetLightBoxScreen({
    super.key,
    required this.onBack,
    required this.onIHaveOne,
  });

  @override
  State<_GetLightBoxScreen> createState() => _GetLightBoxScreenState();
}

class _GetLightBoxScreenState extends State<_GetLightBoxScreen>
    with TickerProviderStateMixin {
  late final AnimationController _reveal;
  late final AnimationController _twinkle;
  late final AnimationController _beat;

  @override
  void initState() {
    super.initState();
    AnalyticsService().logScreenView('onboarding_lightbox_upsell');

    _reveal = AnimationController(
      duration: const Duration(milliseconds: 1500),
      vsync: this,
    );
    WidgetsBinding.instance.addPostFrameCallback((_) => _reveal.forward());

    _twinkle = AnimationController(
      duration: const Duration(seconds: 9),
      vsync: this,
    )..repeat();

    _beat = AnimationController(
      duration: const Duration(milliseconds: 3200),
      vsync: this,
    )..repeat(reverse: true);
  }

  @override
  void dispose() {
    _reveal.dispose();
    _twinkle.dispose();
    _beat.dispose();
    super.dispose();
  }

  Future<void> _openShop() async {
    HapticFeedback.mediumImpact();
    AnalyticsService().logEvent('onboarding_lightbox_shop_opened');
    final uri = Uri.parse('https://rhythm.lighting');
    await launchUrl(uri, mode: LaunchMode.externalApplication);
  }

  @override
  Widget build(BuildContext context) {
    return ColoredBox(
      color: CelestialColors.backgroundDark,
      child: Stack(
        fit: StackFit.expand,
        children: [
          const _AuroraGlow(),
          AnimatedBuilder(
            animation: _twinkle,
            builder: (context, _) =>
                CustomPaint(painter: _StarfieldPainter(t: _twinkle.value)),
          ),
          const _HorizonGlow(),
          SafeArea(
            child: Column(
              children: [
                // Top bar with back chevron
                _Reveal(
                  controller: _reveal,
                  start: 0.0,
                  child: _UpsellTopBar(onBack: widget.onBack),
                ),
                Expanded(
                  child: SingleChildScrollView(
                    physics: const BouncingScrollPhysics(),
                    padding: const EdgeInsets.fromLTRB(28, 8, 28, 8),
                    child: Column(
                      children: [
                        const SizedBox(height: 4),
                        _Reveal(
                          controller: _reveal,
                          start: 0.06,
                          child: const _Eyebrow(text: 'INTRODUCING'),
                        ),
                        const SizedBox(height: 18),
                        _Reveal(
                          controller: _reveal,
                          start: 0.14,
                          child: const _LightBoxWordmark(),
                        ),
                        const SizedBox(height: 14),
                        _Reveal(
                          controller: _reveal,
                          start: 0.22,
                          child: const _UpsellTagline(),
                        ),
                        const SizedBox(height: 28),
                        _Reveal(
                          controller: _reveal,
                          start: 0.32,
                          child: AnimatedBuilder(
                            animation: _beat,
                            builder: (context, _) => SizedBox(
                              height: 230,
                              child: CustomPaint(
                                size: const Size.fromHeight(230),
                                painter: _LightBoxBeaconPainter(
                                  beat: _beat.value,
                                  mini: false,
                                ),
                              ),
                            ),
                          ),
                        ),
                        const SizedBox(height: 32),
                        _Reveal(
                          controller: _reveal,
                          start: 0.46,
                          child: const _FeatureRow(
                            glyph: _FeatureGlyph.curve,
                            title: 'Adaptive curves',
                            body:
                                'Light that follows your body, not the clock.',
                          ),
                        ),
                        const SizedBox(height: 16),
                        _Reveal(
                          controller: _reveal,
                          start: 0.55,
                          child: const _FeatureRow(
                            glyph: _FeatureGlyph.network,
                            title: 'Whole-home rhythm',
                            body:
                                'Hue, Home Assistant, and Matter — pulled into a single circadian loop.',
                          ),
                        ),
                        const SizedBox(height: 16),
                        _Reveal(
                          controller: _reveal,
                          start: 0.64,
                          child: const _FeatureRow(
                            glyph: _FeatureGlyph.local,
                            title: '100% local',
                            body: 'Runs on your network.',
                          ),
                        ),
                        const SizedBox(height: 28),
                      ],
                    ),
                  ),
                ),
                _Reveal(
                  controller: _reveal,
                  start: 0.74,
                  child: Padding(
                    padding: const EdgeInsets.fromLTRB(28, 0, 28, 4),
                    child: _GetOneButton(onTap: _openShop),
                  ),
                ),
                const SizedBox(height: 10),
                _Reveal(
                  controller: _reveal,
                  start: 0.84,
                  child: _AlreadyHaveOneLink(onTap: widget.onIHaveOne),
                ),
                const SizedBox(height: 18),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class _UpsellTopBar extends StatelessWidget {
  final VoidCallback onBack;
  const _UpsellTopBar({required this.onBack});

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(8, 8, 8, 0),
      child: Row(
        children: [
          IconButton(
            onPressed: onBack,
            splashRadius: 22,
            icon: Icon(
              Icons.arrow_back_rounded,
              color: CelestialColors.textSecondary.withValues(alpha: 0.85),
              size: 22,
            ),
          ),
          const Spacer(),
        ],
      ),
    );
  }
}

class _LightBoxWordmark extends StatelessWidget {
  const _LightBoxWordmark();

  @override
  Widget build(BuildContext context) {
    return ShaderMask(
      shaderCallback: (rect) => ui.Gradient.linear(
        Offset(rect.left, 0),
        Offset(rect.right, 0),
        const [
          CelestialColors.sunWarm,
          Color(0xFFFFE0B2),
          Color(0xFF8AB4F8),
        ],
        const [0.0, 0.5, 1.0],
      ),
      blendMode: BlendMode.srcIn,
      child: const Text(
        'LightBox',
        style: TextStyle(
          color: Colors.white,
          fontSize: 52,
          fontWeight: FontWeight.w300,
          letterSpacing: -0.8,
          height: 1.0,
        ),
      ),
    );
  }
}

class _UpsellTagline extends StatelessWidget {
  const _UpsellTagline();

  @override
  Widget build(BuildContext context) {
    return Text(
      'The brain that gives any\nsmart bulb the rhythm of the sun.',
      textAlign: TextAlign.center,
      style: TextStyle(
        color: CelestialColors.textSecondary.withValues(alpha: 0.85),
        fontSize: 15,
        fontWeight: FontWeight.w400,
        letterSpacing: 0.2,
        height: 1.5,
      ),
    );
  }
}

enum _FeatureGlyph { curve, network, local }

class _FeatureRow extends StatelessWidget {
  final _FeatureGlyph glyph;
  final String title;
  final String body;

  const _FeatureRow({
    required this.glyph,
    required this.title,
    required this.body,
  });

  @override
  Widget build(BuildContext context) {
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        SizedBox(
          width: 44,
          height: 44,
          child: CustomPaint(painter: _FeatureGlyphPainter(glyph: glyph)),
        ),
        const SizedBox(width: 16),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(
                title,
                style: TextStyle(
                  color: CelestialColors.textPrimary,
                  fontSize: 15.5,
                  fontWeight: FontWeight.w600,
                  letterSpacing: 0.1,
                ),
              ),
              const SizedBox(height: 3),
              Text(
                body,
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.75),
                  fontSize: 13,
                  fontWeight: FontWeight.w400,
                  letterSpacing: 0.1,
                  height: 1.45,
                ),
              ),
            ],
          ),
        ),
      ],
    );
  }
}

class _FeatureGlyphPainter extends CustomPainter {
  final _FeatureGlyph glyph;
  _FeatureGlyphPainter({required this.glyph});

  @override
  void paint(Canvas canvas, Size size) {
    final c = Offset(size.width / 2, size.height / 2);
    final r = size.width * 0.46;

    // Soft tinted disk behind the glyph.
    final tint = switch (glyph) {
      _FeatureGlyph.curve => CelestialColors.sunWarm,
      _FeatureGlyph.network => const Color(0xFF8AB4F8),
      _FeatureGlyph.local => const Color(0xFF7FD1A0),
    };

    final disk = Paint()
      ..shader = ui.Gradient.radial(c, r, [
        tint.withValues(alpha: 0.20),
        tint.withValues(alpha: 0.04),
      ]);
    canvas.drawCircle(c, r, disk);

    final ring = Paint()
      ..color = tint.withValues(alpha: 0.45)
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.0;
    canvas.drawCircle(c, r, ring);

    final stroke = Paint()
      ..color = tint
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.6
      ..strokeCap = StrokeCap.round;

    switch (glyph) {
      case _FeatureGlyph.curve:
        // Sine-like rhythm curve.
        final path = Path();
        final left = c.dx - r * 0.66;
        final right = c.dx + r * 0.66;
        final w = right - left;
        path.moveTo(left, c.dy);
        for (var x = 0.0; x <= w; x += 1) {
          final t = x / w;
          final y = c.dy - math.sin(t * 2 * math.pi) * r * 0.32;
          path.lineTo(left + x, y);
        }
        canvas.drawPath(path, stroke);

        // Tiny sun dot at the peak.
        final peakX = left + w * 0.25;
        final peakY = c.dy - r * 0.32;
        canvas.drawCircle(
          Offset(peakX, peakY),
          2.4,
          Paint()..color = tint,
        );
        break;

      case _FeatureGlyph.network:
        // Three nodes connected to a center.
        final nodes = [
          Offset(c.dx, c.dy - r * 0.55),
          Offset(c.dx - r * 0.55, c.dy + r * 0.30),
          Offset(c.dx + r * 0.55, c.dy + r * 0.30),
        ];
        for (final n in nodes) {
          canvas.drawLine(c, n, stroke);
          canvas.drawCircle(n, 2.6, Paint()..color = tint);
        }
        canvas.drawCircle(
          c,
          3.2,
          Paint()..color = Colors.white.withValues(alpha: 0.92),
        );
        break;

      case _FeatureGlyph.local:
        // House silhouette.
        final path = Path()
          ..moveTo(c.dx - r * 0.55, c.dy + r * 0.45)
          ..lineTo(c.dx - r * 0.55, c.dy - r * 0.05)
          ..lineTo(c.dx, c.dy - r * 0.55)
          ..lineTo(c.dx + r * 0.55, c.dy - r * 0.05)
          ..lineTo(c.dx + r * 0.55, c.dy + r * 0.45)
          ..close();
        canvas.drawPath(path, stroke);
        // Glowing dot inside.
        canvas.drawCircle(
          Offset(c.dx, c.dy + r * 0.05),
          3.2,
          Paint()
            ..color = tint
            ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 2),
        );
        break;
    }
  }

  @override
  bool shouldRepaint(covariant _FeatureGlyphPainter oldDelegate) =>
      oldDelegate.glyph != glyph;
}

class _GetOneButton extends StatefulWidget {
  final VoidCallback onTap;
  const _GetOneButton({required this.onTap});

  @override
  State<_GetOneButton> createState() => _GetOneButtonState();
}

class _GetOneButtonState extends State<_GetOneButton>
    with SingleTickerProviderStateMixin {
  late final AnimationController _glow;
  bool _pressed = false;

  @override
  void initState() {
    super.initState();
    _glow = AnimationController(
      duration: const Duration(milliseconds: 2200),
      vsync: this,
    )..repeat(reverse: true);
  }

  @override
  void dispose() {
    _glow.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: _glow,
      builder: (context, _) {
        final b = _glow.value;
        return GestureDetector(
          onTapDown: (_) => setState(() => _pressed = true),
          onTapUp: (_) => setState(() => _pressed = false),
          onTapCancel: () => setState(() => _pressed = false),
          onTap: widget.onTap,
          child: AnimatedScale(
            duration: const Duration(milliseconds: 110),
            scale: _pressed ? 0.98 : 1.0,
            child: Container(
              padding: const EdgeInsets.symmetric(vertical: 18),
              decoration: BoxDecoration(
                borderRadius: BorderRadius.circular(18),
                gradient: LinearGradient(
                  begin: Alignment.topLeft,
                  end: Alignment.bottomRight,
                  colors: [
                    CelestialColors.sunWarm,
                    const Color(0xFFE8821C),
                  ],
                ),
                boxShadow: [
                  BoxShadow(
                    color: CelestialColors.sunWarm
                        .withValues(alpha: 0.32 + 0.20 * b),
                    blurRadius: 32 + 10 * b,
                    spreadRadius: -3,
                    offset: const Offset(0, 10),
                  ),
                ],
              ),
              child: Row(
                mainAxisAlignment: MainAxisAlignment.center,
                children: [
                  const Text(
                    'Get a LightBox',
                    style: TextStyle(
                      color: Color(0xFF1A1206),
                      fontSize: 17,
                      fontWeight: FontWeight.w700,
                      letterSpacing: 0.3,
                    ),
                  ),
                  const SizedBox(width: 8),
                  Icon(
                    Icons.north_east_rounded,
                    color: const Color(0xFF1A1206).withValues(alpha: 0.85),
                    size: 19,
                  ),
                ],
              ),
            ),
          ),
        );
      },
    );
  }
}

class _AlreadyHaveOneLink extends StatelessWidget {
  final VoidCallback onTap;
  const _AlreadyHaveOneLink({required this.onTap});

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: onTap,
      behavior: HitTestBehavior.opaque,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
        child: Text(
          'I already have one',
          style: TextStyle(
            color: CelestialColors.textSecondary.withValues(alpha: 0.78),
            fontSize: 13.5,
            fontWeight: FontWeight.w500,
            letterSpacing: 0.4,
            decoration: TextDecoration.underline,
            decorationColor:
                CelestialColors.textSecondary.withValues(alpha: 0.4),
          ),
        ),
      ),
    );
  }
}

// ═══════════════════════════════════════════════════════════════════════════
// Shared atmosphere painters — copied/adapted from welcome_screen.dart so this
// module can stand alone without coupling to onboarding/.
// ═══════════════════════════════════════════════════════════════════════════

class _Eyebrow extends StatelessWidget {
  final String text;
  const _Eyebrow({required this.text});

  @override
  Widget build(BuildContext context) {
    return Row(
      mainAxisAlignment: MainAxisAlignment.center,
      children: [
        Container(
          width: 5,
          height: 5,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: CelestialColors.sunWarm,
            boxShadow: [
              BoxShadow(
                color: CelestialColors.sunWarm.withValues(alpha: 0.6),
                blurRadius: 8,
                spreadRadius: 1,
              ),
            ],
          ),
        ),
        const SizedBox(width: 10),
        Text(
          text,
          style: TextStyle(
            color: CelestialColors.textPrimary.withValues(alpha: 0.7),
            fontSize: 10.5,
            fontWeight: FontWeight.w700,
            letterSpacing: 3.6,
          ),
        ),
      ],
    );
  }
}

class _Reveal extends StatelessWidget {
  final AnimationController controller;
  final double start;
  final Widget child;

  const _Reveal({
    required this.controller,
    required this.start,
    required this.child,
  });

  @override
  Widget build(BuildContext context) {
    final curve = CurvedAnimation(
      parent: controller,
      curve: Interval(
        start,
        math.min(1.0, start + 0.5),
        curve: Curves.easeOutCubic,
      ),
    );
    return AnimatedBuilder(
      animation: curve,
      builder: (context, child) {
        final v = curve.value;
        return Opacity(
          opacity: v,
          child: Transform.translate(
            offset: Offset(0, (1 - v) * 12),
            child: child,
          ),
        );
      },
      child: child,
    );
  }
}

class _AuroraGlow extends StatelessWidget {
  const _AuroraGlow();

  @override
  Widget build(BuildContext context) {
    return IgnorePointer(
      child: Stack(
        fit: StackFit.expand,
        children: [
          DecoratedBox(
            decoration: BoxDecoration(
              gradient: RadialGradient(
                center: const Alignment(0, -0.3),
                radius: 0.95,
                colors: [
                  CelestialColors.sunWarm.withValues(alpha: 0.13),
                  CelestialColors.sunWarm.withValues(alpha: 0.02),
                  Colors.transparent,
                ],
                stops: const [0.0, 0.45, 1.0],
              ),
            ),
          ),
          DecoratedBox(
            decoration: BoxDecoration(
              gradient: RadialGradient(
                center: const Alignment(0.7, 0.7),
                radius: 0.9,
                colors: [
                  const Color(0xFF2D3A5C).withValues(alpha: 0.55),
                  const Color(0xFF2D3A5C).withValues(alpha: 0.10),
                  Colors.transparent,
                ],
                stops: const [0.0, 0.5, 1.0],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _HorizonGlow extends StatelessWidget {
  const _HorizonGlow();

  @override
  Widget build(BuildContext context) {
    return IgnorePointer(
      child: Align(
        alignment: Alignment.bottomCenter,
        child: FractionallySizedBox(
          widthFactor: 1.0,
          heightFactor: 0.30,
          child: DecoratedBox(
            decoration: BoxDecoration(
              gradient: LinearGradient(
                begin: Alignment.bottomCenter,
                end: Alignment.topCenter,
                colors: [
                  CelestialColors.sunWarm.withValues(alpha: 0.07),
                  CelestialColors.sunWarm.withValues(alpha: 0.015),
                  Colors.transparent,
                ],
                stops: const [0.0, 0.45, 1.0],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _StarfieldPainter extends CustomPainter {
  final double t;
  static const _seed = 23;
  static const _count = 90;

  _StarfieldPainter({required this.t});

  @override
  void paint(Canvas canvas, Size size) {
    final rng = math.Random(_seed);
    for (var i = 0; i < _count; i++) {
      final x = rng.nextDouble() * size.width;
      final y = rng.nextDouble() * size.height;
      final base = 0.16 + rng.nextDouble() * 0.5;
      final phase = rng.nextDouble() * math.pi * 2;
      final twinkle = (math.sin(t * 2 * math.pi + phase) + 1) / 2;
      final radius = 0.5 + rng.nextDouble() * 1.4;
      final alpha = (base * (0.5 + 0.5 * twinkle)).clamp(0.0, 0.85);

      final tint = i % 17 == 0
          ? CelestialColors.sunWarm
          : (i % 11 == 0 ? const Color(0xFF8AB4F8) : Colors.white);

      final paint = Paint()
        ..color = tint.withValues(alpha: alpha)
        ..maskFilter =
            radius > 1.2 ? const MaskFilter.blur(BlurStyle.normal, 1.2) : null;
      canvas.drawCircle(Offset(x, y), radius, paint);
    }
  }

  @override
  bool shouldRepaint(covariant _StarfieldPainter oldDelegate) =>
      oldDelegate.t != t;
}

// ═══════════════════════════════════════════════════════════════════════════
// LightBox beacon — the hero illustration. A glowing rectangular slab that
// projects a volumetric light shaft upward, with a wash of warm-to-cool
// light and a soft rhythm pulse on the top edge. The same painter is used
// at small scale on the gate and full size on the upsell.
// ═══════════════════════════════════════════════════════════════════════════

class _LightBoxBeaconPainter extends CustomPainter {
  final double beat; // 0..1
  final bool mini;
  _LightBoxBeaconPainter({required this.beat, required this.mini});

  @override
  void paint(Canvas canvas, Size size) {
    final w = size.width;
    final h = size.height;

    final boxW = w * (mini ? 0.62 : 0.72);
    final boxH = h * (mini ? 0.16 : 0.13);
    final boxRect = Rect.fromCenter(
      center: Offset(w / 2, h * 0.78),
      width: boxW,
      height: boxH,
    );

    final shaftBottom = boxRect.top + 1;
    final shaftTopY = h * 0.05;

    // ── Light shaft (volumetric beam from the box upward) ──
    final shaftPath = Path()
      ..moveTo(boxRect.left + boxW * 0.18, shaftBottom)
      ..lineTo(w / 2 - boxW * 0.10, shaftTopY)
      ..lineTo(w / 2 + boxW * 0.10, shaftTopY)
      ..lineTo(boxRect.right - boxW * 0.18, shaftBottom)
      ..close();

    final shaftPaint = Paint()
      ..shader = ui.Gradient.linear(
        Offset(w / 2, shaftBottom),
        Offset(w / 2, shaftTopY),
        [
          CelestialColors.sunWarm.withValues(alpha: 0.42 + 0.15 * beat),
          CelestialColors.sunWarm.withValues(alpha: 0.10 + 0.05 * beat),
          const Color(0xFF8AB4F8).withValues(alpha: 0.06),
          Colors.transparent,
        ],
        const [0.0, 0.35, 0.75, 1.0],
      )
      ..maskFilter = MaskFilter.blur(BlurStyle.normal, mini ? 8 : 12);
    canvas.drawPath(shaftPath, shaftPaint);

    // ── Sharper inner shaft for definition ──
    final innerShaftPath = Path()
      ..moveTo(boxRect.left + boxW * 0.30, shaftBottom)
      ..lineTo(w / 2 - boxW * 0.04, shaftTopY + 14)
      ..lineTo(w / 2 + boxW * 0.04, shaftTopY + 14)
      ..lineTo(boxRect.right - boxW * 0.30, shaftBottom)
      ..close();

    final innerPaint = Paint()
      ..shader = ui.Gradient.linear(
        Offset(w / 2, shaftBottom),
        Offset(w / 2, shaftTopY),
        [
          Colors.white.withValues(alpha: 0.30 + 0.20 * beat),
          CelestialColors.sunWarm.withValues(alpha: 0.18 + 0.12 * beat),
          Colors.transparent,
        ],
        const [0.0, 0.4, 1.0],
      )
      ..maskFilter = MaskFilter.blur(BlurStyle.normal, mini ? 4 : 6);
    canvas.drawPath(innerShaftPath, innerPaint);

    // ── A ring of dust motes drifting up the beam ──
    final rng = math.Random(7);
    for (var i = 0; i < (mini ? 16 : 28); i++) {
      final t = ((rng.nextDouble() + beat * 0.5) % 1.0);
      final yLerp = boxRect.top - t * (boxRect.top - shaftTopY);
      final spread = ui.lerpDouble(boxW * 0.34, boxW * 0.05, t)!;
      final x = w / 2 + (rng.nextDouble() * 2 - 1) * spread;
      final r = (rng.nextDouble() * 1.2 + 0.4) * (1 - t * 0.4);
      final a = (1 - t) * 0.55;
      canvas.drawCircle(
        Offset(x, yLerp),
        r,
        Paint()
          ..color = CelestialColors.sunWarm.withValues(alpha: a.clamp(0.0, 0.7))
          ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 1.2),
      );
    }

    // ── Underglow on the floor (warm puddle of light) ──
    final glowRect = Rect.fromCenter(
      center: Offset(w / 2, boxRect.bottom + boxH * 0.6),
      width: boxW * 1.7,
      height: boxH * 1.4,
    );
    canvas.drawOval(
      glowRect,
      Paint()
        ..shader = ui.Gradient.radial(
          glowRect.center,
          glowRect.width / 2,
          [
            CelestialColors.sunWarm.withValues(alpha: 0.32 + 0.10 * beat),
            CelestialColors.sunWarm.withValues(alpha: 0.06),
            Colors.transparent,
          ],
          const [0.0, 0.55, 1.0],
        ),
    );

    // ── Box body — dark slab with subtle bevel ──
    final bodyRRect = RRect.fromRectAndRadius(
      boxRect,
      Radius.circular(boxH * 0.28),
    );
    canvas.drawRRect(
      bodyRRect,
      Paint()
        ..shader = ui.Gradient.linear(
          boxRect.topLeft,
          boxRect.bottomLeft,
          const [
            Color(0xFF1B2230),
            Color(0xFF0F1420),
          ],
        ),
    );
    canvas.drawRRect(
      bodyRRect,
      Paint()
        ..color = Colors.white.withValues(alpha: 0.06)
        ..style = PaintingStyle.stroke
        ..strokeWidth = 1.0,
    );

    // ── Hot top edge (the emitter) — a rounded line of light ──
    final emitterRect = Rect.fromLTWH(
      boxRect.left + boxH * 0.4,
      boxRect.top - boxH * 0.05,
      boxW - boxH * 0.8,
      boxH * 0.18,
    );
    final emitterRRect =
        RRect.fromRectAndRadius(emitterRect, Radius.circular(boxH * 0.12));

    canvas.drawRRect(
      emitterRRect.inflate(2.5),
      Paint()
        ..color = CelestialColors.sunWarm.withValues(alpha: 0.55 + 0.30 * beat)
        ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 6),
    );
    canvas.drawRRect(
      emitterRRect,
      Paint()
        ..shader = ui.Gradient.linear(
          emitterRect.topLeft,
          emitterRect.topRight,
          const [
            Color(0xFFFFE3A6),
            Color(0xFFFFB54A),
            Color(0xFFFFE3A6),
          ],
          const [0.0, 0.5, 1.0],
        ),
    );

    // ── Tiny status pip on the front face — a single live LED ──
    final pipR = boxH * 0.07;
    final pipPos =
        Offset(boxRect.left + boxH * 0.55, boxRect.center.dy + boxH * 0.08);
    canvas.drawCircle(
      pipPos,
      pipR * 2.5,
      Paint()
        ..color = const Color(0xFF7FD1A0).withValues(alpha: 0.55 + 0.30 * beat)
        ..maskFilter = const MaskFilter.blur(BlurStyle.normal, 2),
    );
    canvas.drawCircle(
      pipPos,
      pipR,
      Paint()..color = const Color(0xFFB6F1CE),
    );

    // ── Subtle "rhythm" wordmark engraved on the box (only at full size) ──
    if (!mini) {
      final tp = TextPainter(
        text: TextSpan(
          text: 'rhythm',
          style: TextStyle(
            color: Colors.white.withValues(alpha: 0.18),
            fontSize: boxH * 0.32,
            fontWeight: FontWeight.w300,
            letterSpacing: 1.2,
          ),
        ),
        textDirection: TextDirection.ltr,
      )..layout();
      tp.paint(
        canvas,
        Offset(
          boxRect.center.dx - tp.width / 2,
          boxRect.center.dy + boxH * 0.06,
        ),
      );
    }
  }

  @override
  bool shouldRepaint(covariant _LightBoxBeaconPainter oldDelegate) =>
      oldDelegate.beat != beat || oldDelegate.mini != mini;
}
