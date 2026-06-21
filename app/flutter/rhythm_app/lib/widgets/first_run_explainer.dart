import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../data/local_data_source.dart';
import 'solar_orbit.dart' show CelestialColors;

/// One illustrated talking point inside a [FirstRunExplainer].
///
/// Each point is rendered as a step in a connected list: an accent-tinted icon
/// chip threaded to the next by a hairline, a short [title], and a plain-spoken
/// [body]. Keep the copy beginner-friendly — one idea per point.
class ExplainerPoint {
  final IconData icon;
  final String title;
  final String body;

  const ExplainerPoint({
    required this.icon,
    required this.title,
    required this.body,
  });
}

/// A reusable, one-time concept explainer presented as a slide-up sheet.
///
/// Use it the *first* time a user reaches for an unfamiliar feature: it
/// introduces the idea calmly, in plain language, then steps out of the way.
/// Once shown for a given [id] it never appears again (persisted device-local
/// via [LocalDataSource]), so callers can simply gate an action behind
/// [maybeShow] without tracking their own flags.
///
/// ```dart
/// await FirstRunExplainer.maybeShow(
///   context,
///   id: 'mood',
///   eyebrow: 'NEW',
///   title: 'Meet Mood',
///   subtitle: 'A calm, hand-picked light that stays just how you set it.',
///   icon: Icons.spa_rounded,
///   accent: const Color(0xFFFFB23E),
///   points: const [
///     ExplainerPoint(icon: Icons.palette_rounded, title: '…', body: '…'),
///   ],
///   ctaLabel: 'Choose a Mood',
/// );
/// ```
///
/// Visual language follows the app's Celestial palette and the [MoodSheet]: a
/// deep surface with an ambient accent glow, a drag handle, a hero badge, and a
/// staggered reveal so the sheet feels composed rather than dumped on screen.
class FirstRunExplainer extends StatefulWidget {
  /// Short tracked-uppercase kicker above the title (e.g. "NEW"). Hidden when
  /// null.
  final String? eyebrow;

  /// The headline — name the concept (e.g. "Meet Mood").
  final String title;

  /// A single warm sentence framing the concept. Hidden when null.
  final String? subtitle;

  /// Hero glyph shown in the badge. Pick the same icon the feature uses.
  final IconData icon;

  /// Drives the glow, badge, eyebrow, stepper thread, and CTA. Let each
  /// explainer carry the color of the thing it explains.
  final Color accent;

  /// The talking points, rendered as a connected step list.
  final List<ExplainerPoint> points;

  /// Label for the single confirming action.
  final String ctaLabel;

  const FirstRunExplainer({
    super.key,
    required this.title,
    required this.icon,
    required this.points,
    this.accent = CelestialColors.accentBlue,
    this.eyebrow,
    this.subtitle,
    this.ctaLabel = 'Got it',
  });

  // --- Persistence ---------------------------------------------------------

  static const String _keyPrefix = 'explainer_seen.';

  static String _storageKey(String id) => '$_keyPrefix$id';

  /// Test seam: read whether the explainer for [id] has already been shown.
  /// Defaults to the Hive-backed settings box; override in widget tests.
  static bool Function(String id) seenReader = _defaultHasSeen;

  /// Test seam: persist that the explainer for [id] has been shown.
  static Future<void> Function(String id) seenWriter = _defaultMarkSeen;

  static bool _defaultHasSeen(String id) {
    final ds = LocalDataSource();
    if (!ds.isInitialized) return false;
    return ds.getSettingsValue(_storageKey(id)) == true;
  }

  static Future<void> _defaultMarkSeen(String id) async {
    final ds = LocalDataSource();
    if (!ds.isInitialized) return;
    await ds.saveSettingsValue(_storageKey(id), true);
  }

  /// Whether the explainer for [id] has already been seen.
  static bool hasSeen(String id) => seenReader(id);

  /// Mark the explainer for [id] as seen without showing it.
  static Future<void> markSeen(String id) => seenWriter(id);

  /// Show the explainer for [id] only if it hasn't been seen before.
  ///
  /// Returns immediately (with no UI) when already seen, so it is safe to
  /// `await` on every interaction. The "seen" flag is written the moment the
  /// sheet is shown, so it never double-presents even if dismissed instantly.
  static Future<void> maybeShow(
    BuildContext context, {
    required String id,
    required String title,
    required IconData icon,
    required List<ExplainerPoint> points,
    Color accent = CelestialColors.accentBlue,
    String? eyebrow,
    String? subtitle,
    String ctaLabel = 'Got it',
  }) {
    if (hasSeen(id)) return Future<void>.value();
    return show(
      context,
      id: id,
      title: title,
      icon: icon,
      points: points,
      accent: accent,
      eyebrow: eyebrow,
      subtitle: subtitle,
      ctaLabel: ctaLabel,
    );
  }

  /// Show the explainer unconditionally. When [id] is given, it is marked seen
  /// (so a forced preview also satisfies a later [maybeShow]).
  static Future<void> show(
    BuildContext context, {
    String? id,
    required String title,
    required IconData icon,
    required List<ExplainerPoint> points,
    Color accent = CelestialColors.accentBlue,
    String? eyebrow,
    String? subtitle,
    String ctaLabel = 'Got it',
  }) async {
    if (id != null) await markSeen(id);
    if (!context.mounted) return;
    await showModalBottomSheet<void>(
      context: context,
      isScrollControlled: true,
      backgroundColor: Colors.transparent,
      barrierColor: Colors.black.withValues(alpha: 0.62),
      builder: (_) => FirstRunExplainer(
        title: title,
        icon: icon,
        points: points,
        accent: accent,
        eyebrow: eyebrow,
        subtitle: subtitle,
        ctaLabel: ctaLabel,
      ),
    );
  }

  @override
  State<FirstRunExplainer> createState() => _FirstRunExplainerState();
}

class _FirstRunExplainerState extends State<FirstRunExplainer>
    with TickerProviderStateMixin {
  static const Color _bg = Color(0xFF12151D);

  late final AnimationController _enter = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 760),
  )..forward();

  late final AnimationController _glow = AnimationController(
    vsync: this,
    duration: const Duration(seconds: 4),
  )..repeat(reverse: true);

  @override
  void dispose() {
    _enter.dispose();
    _glow.dispose();
    super.dispose();
  }

  /// Eased reveal for the slice of the timeline between [start] and [end].
  double _reveal(double start, double end) {
    final raw = ((_enter.value - start) / (end - start)).clamp(0.0, 1.0);
    return Curves.easeOutCubic.transform(raw);
  }

  Widget _staggered(double start, double end, Widget child) {
    return AnimatedBuilder(
      animation: _enter,
      builder: (context, inner) {
        final t = _reveal(start, end);
        return Opacity(
          opacity: t,
          child: Transform.translate(
            offset: Offset(0, (1 - t) * 14),
            child: inner,
          ),
        );
      },
      child: child,
    );
  }

  @override
  Widget build(BuildContext context) {
    final accent = widget.accent;
    final bottomPad = MediaQuery.of(context).padding.bottom;
    final maxHeight = MediaQuery.of(context).size.height * 0.9;
    final points = widget.points;

    // Points reveal across the middle of the timeline, one trailing the next.
    const pointsStart = 0.32;
    const pointsEnd = 0.86;
    final perPoint =
        points.isEmpty ? 0.0 : (pointsEnd - pointsStart) / points.length;

    return ConstrainedBox(
      constraints: BoxConstraints(maxHeight: maxHeight),
      child: Container(
        decoration: const BoxDecoration(
          color: _bg,
          borderRadius: BorderRadius.vertical(top: Radius.circular(28)),
        ),
        child: Stack(
          children: [
            // Ambient accent glow breathing at the top, like the mood sheet.
            Positioned.fill(
              child: IgnorePointer(
                child: AnimatedBuilder(
                  animation: _glow,
                  builder: (context, _) {
                    final pulse = 0.75 + _glow.value * 0.25;
                    return DecoratedBox(
                      decoration: BoxDecoration(
                        borderRadius: const BorderRadius.vertical(
                          top: Radius.circular(28),
                        ),
                        gradient: RadialGradient(
                          center: const Alignment(0.0, -1.0),
                          radius: 1.4,
                          colors: [
                            accent.withValues(alpha: 0.20 * pulse),
                            accent.withValues(alpha: 0.05 * pulse),
                            Colors.transparent,
                          ],
                          stops: const [0.0, 0.42, 1.0],
                        ),
                      ),
                    );
                  },
                ),
              ),
            ),
            SafeArea(
              top: false,
              child: SingleChildScrollView(
                padding: EdgeInsets.fromLTRB(24, 12, 24, 20 + bottomPad),
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    Center(child: _dragHandle()),
                    const SizedBox(height: 22),
                    _staggered(0.0, 0.5, _HeroBadge(icon: widget.icon, accent: accent)),
                    const SizedBox(height: 18),
                    if (widget.eyebrow != null) ...[
                      _staggered(0.08, 0.56, _Eyebrow(text: widget.eyebrow!, accent: accent)),
                      const SizedBox(height: 10),
                    ],
                    _staggered(
                      0.12,
                      0.6,
                      Text(
                        widget.title,
                        textAlign: TextAlign.center,
                        style: const TextStyle(
                          color: CelestialColors.textPrimary,
                          fontSize: 25,
                          fontWeight: FontWeight.w700,
                          letterSpacing: 0.2,
                          height: 1.1,
                        ),
                      ),
                    ),
                    if (widget.subtitle != null) ...[
                      const SizedBox(height: 10),
                      _staggered(
                        0.18,
                        0.66,
                        Padding(
                          padding: const EdgeInsets.symmetric(horizontal: 6),
                          child: Text(
                            widget.subtitle!,
                            textAlign: TextAlign.center,
                            style: const TextStyle(
                              color: CelestialColors.textSecondary,
                              fontSize: 14.5,
                              height: 1.45,
                              letterSpacing: 0.1,
                              fontWeight: FontWeight.w400,
                            ),
                          ),
                        ),
                      ),
                    ],
                    const SizedBox(height: 28),
                    for (var i = 0; i < points.length; i++)
                      _staggered(
                        pointsStart + perPoint * i,
                        (pointsStart + perPoint * i + 0.4).clamp(0.0, 1.0),
                        _PointRow(
                          point: points[i],
                          accent: accent,
                          isLast: i == points.length - 1,
                        ),
                      ),
                    const SizedBox(height: 28),
                    _staggered(
                      0.7,
                      1.0,
                      _CtaButton(
                        label: widget.ctaLabel,
                        accent: accent,
                        onTap: () {
                          HapticFeedback.mediumImpact();
                          Navigator.of(context).maybePop();
                        },
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _dragHandle() {
    return Container(
      width: 40,
      height: 4,
      decoration: BoxDecoration(
        color: Colors.white.withValues(alpha: 0.18),
        borderRadius: BorderRadius.circular(2),
      ),
    );
  }
}

/// Glowing rounded badge holding the feature's glyph.
class _HeroBadge extends StatelessWidget {
  final IconData icon;
  final Color accent;

  const _HeroBadge({required this.icon, required this.accent});

  @override
  Widget build(BuildContext context) {
    return Center(
      child: Container(
        width: 66,
        height: 66,
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(20),
          gradient: LinearGradient(
            begin: Alignment.topLeft,
            end: Alignment.bottomRight,
            colors: [
              accent.withValues(alpha: 0.26),
              accent.withValues(alpha: 0.08),
            ],
          ),
          border: Border.all(color: accent.withValues(alpha: 0.4), width: 1),
          boxShadow: [
            BoxShadow(
              color: accent.withValues(alpha: 0.28),
              blurRadius: 26,
              spreadRadius: -4,
            ),
          ],
        ),
        child: Icon(icon, size: 32, color: accent),
      ),
    );
  }
}

/// Tracked-uppercase kicker with a small glowing dot.
class _Eyebrow extends StatelessWidget {
  final String text;
  final Color accent;

  const _Eyebrow({required this.text, required this.accent});

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
            color: accent.withValues(alpha: 0.9),
            boxShadow: [
              BoxShadow(color: accent.withValues(alpha: 0.6), blurRadius: 6),
            ],
          ),
        ),
        const SizedBox(width: 8),
        Text(
          text,
          style: TextStyle(
            fontSize: 10,
            fontWeight: FontWeight.w700,
            letterSpacing: 2.0,
            color: accent.withValues(alpha: 0.95),
          ),
        ),
      ],
    );
  }
}

/// A single talking point: an icon chip threaded to the next, plus copy.
class _PointRow extends StatelessWidget {
  final ExplainerPoint point;
  final Color accent;
  final bool isLast;

  const _PointRow({
    required this.point,
    required this.accent,
    required this.isLast,
  });

  static const double _chip = 40;

  @override
  Widget build(BuildContext context) {
    return IntrinsicHeight(
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Column(
            children: [
              Container(
                width: _chip,
                height: _chip,
                decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(13),
                  color: accent.withValues(alpha: 0.12),
                  border:
                      Border.all(color: accent.withValues(alpha: 0.28), width: 1),
                ),
                child: Icon(point.icon, size: 19, color: accent),
              ),
              if (!isLast)
                Expanded(
                  child: Center(
                    child: Container(
                      width: 1.5,
                      margin: const EdgeInsets.symmetric(vertical: 4),
                      decoration: BoxDecoration(
                        gradient: LinearGradient(
                          begin: Alignment.topCenter,
                          end: Alignment.bottomCenter,
                          colors: [
                            accent.withValues(alpha: 0.32),
                            accent.withValues(alpha: 0.08),
                          ],
                        ),
                      ),
                    ),
                  ),
                ),
            ],
          ),
          const SizedBox(width: 16),
          Expanded(
            child: Padding(
              padding: EdgeInsets.only(bottom: isLast ? 0 : 20, top: 2),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    point.title,
                    style: const TextStyle(
                      color: CelestialColors.textPrimary,
                      fontSize: 15.5,
                      fontWeight: FontWeight.w600,
                      letterSpacing: 0.1,
                      height: 1.2,
                    ),
                  ),
                  const SizedBox(height: 4),
                  Text(
                    point.body,
                    style: const TextStyle(
                      color: CelestialColors.textSecondary,
                      fontSize: 13.5,
                      height: 1.42,
                      letterSpacing: 0.1,
                      fontWeight: FontWeight.w400,
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
}

/// Full-width confirming action with an accent gradient and soft glow.
class _CtaButton extends StatelessWidget {
  final String label;
  final Color accent;
  final VoidCallback onTap;

  const _CtaButton({
    required this.label,
    required this.accent,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    // Pick readable text for whatever accent the caller passes.
    final onAccent = ThemeData.estimateBrightnessForColor(accent) == Brightness.dark
        ? Colors.white
        : const Color(0xFF14161C);

    return Material(
      color: Colors.transparent,
      child: InkWell(
        borderRadius: BorderRadius.circular(16),
        onTap: onTap,
        child: Ink(
          height: 54,
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(16),
            gradient: LinearGradient(
              begin: Alignment.topLeft,
              end: Alignment.bottomRight,
              colors: [
                Color.lerp(accent, Colors.white, 0.12)!,
                accent,
              ],
            ),
            boxShadow: [
              BoxShadow(
                color: accent.withValues(alpha: 0.35),
                blurRadius: 20,
                spreadRadius: -4,
                offset: const Offset(0, 6),
              ),
            ],
          ),
          child: Center(
            child: Text(
              label,
              style: TextStyle(
                color: onAccent,
                fontSize: 15.5,
                fontWeight: FontWeight.w700,
                letterSpacing: 0.3,
              ),
            ),
          ),
        ),
      ),
    );
  }
}
