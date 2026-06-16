import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'solar_orbit.dart' show CelestialColors;

/// A polished, tappable info affordance: a small (i) icon that, when tapped,
/// reveals a card-style tooltip with a caret pointing back to the trigger.
///
/// Visual language matches the Celestial palette — a deep gradient surface,
/// a hairline accent border, layered shadows including a soft blue diffuse
/// halo, and an uppercase tracked "INFO" header. Dismisses on outside tap or
/// after [showDuration].
///
/// Positioning is anchored to the trigger via a [LayerLink] +
/// [CompositedTransformFollower] so the card and caret stay perfectly aligned
/// regardless of layout context or overlay coordinate space.
class InfoTooltip extends StatefulWidget {
  /// Message displayed in the body of the tooltip card.
  final String message;

  /// Optional eyebrow label rendered in tracked uppercase above the message.
  /// Defaults to "INFO" — pass `null` to hide it entirely.
  final String? eyebrow;

  /// Size of the (i) trigger icon. The hit target is enlarged around it.
  final double iconSize;

  /// Resting color of the (i) icon. Defaults to a muted secondary tone.
  final Color? iconColor;

  /// Accent applied to the border, eyebrow, halo, and pulsing dot.
  final Color accentColor;

  /// Card width. The card always renders at this exact width so the caret
  /// (which sits at the horizontal midpoint) lines up reliably with the
  /// trigger.
  final double cardWidth;

  /// How long the tooltip stays open before auto-dismissing.
  final Duration showDuration;

  /// Padding around the card's content.
  final EdgeInsets contentPadding;

  const InfoTooltip({
    super.key,
    required this.message,
    this.eyebrow = 'INFO',
    this.iconSize = 14,
    this.iconColor,
    this.accentColor = CelestialColors.accentBlue,
    this.cardWidth = 264,
    this.showDuration = const Duration(seconds: 6),
    this.contentPadding = const EdgeInsets.fromLTRB(14, 11, 14, 13),
  });

  @override
  State<InfoTooltip> createState() => _InfoTooltipState();
}

class _InfoTooltipState extends State<InfoTooltip>
    with TickerProviderStateMixin {
  static const Color _surfaceTop = Color(0xFF1B2230);
  static const Color _surfaceBottom = Color(0xFF101521);
  static const double _caretHalfWidth = 7;
  static const double _caretHeight = 6;
  static const double _gap = 10;

  final OverlayPortalController _portal = OverlayPortalController();
  final LayerLink _link = LayerLink();
  final GlobalKey _triggerKey = GlobalKey();

  late final AnimationController _anim = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 220),
    reverseDuration: const Duration(milliseconds: 140),
  );
  late final AnimationController _halo = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 1700),
  );
  Timer? _autoDismiss;

  bool _placeAbove = true;

  @override
  void dispose() {
    _autoDismiss?.cancel();
    _anim.dispose();
    _halo.dispose();
    super.dispose();
  }

  void _toggle() {
    if (_portal.isShowing) {
      _hide();
    } else {
      _show();
    }
  }

  void _show() {
    final ctx = _triggerKey.currentContext;
    bool placeAbove = true;
    if (ctx != null) {
      final renderBox = ctx.findRenderObject() as RenderBox?;
      if (renderBox != null && renderBox.attached) {
        final triggerTopGlobal = renderBox.localToGlobal(Offset.zero).dy;
        final mediaTopPadding = MediaQuery.of(context).padding.top;
        // Need at least ~140px of breathing room above the trigger to host the
        // card without crowding the status bar / notch.
        placeAbove = (triggerTopGlobal - mediaTopPadding) > 140;
      }
    }

    setState(() => _placeAbove = placeAbove);

    HapticFeedback.selectionClick();
    _portal.show();
    _halo.repeat(reverse: true);
    _anim.forward(from: 0);

    _autoDismiss?.cancel();
    _autoDismiss = Timer(widget.showDuration, _hide);
  }

  Future<void> _hide() async {
    _autoDismiss?.cancel();
    if (!_portal.isShowing) return;
    await _anim.reverse();
    if (!mounted) return;
    _halo.stop();
    _halo.value = 0;
    _portal.hide();
    if (mounted) setState(() {});
  }

  @override
  Widget build(BuildContext context) {
    return CompositedTransformTarget(
      link: _link,
      child: OverlayPortal(
        controller: _portal,
        overlayChildBuilder: _buildOverlay,
        child: GestureDetector(
          key: _triggerKey,
          behavior: HitTestBehavior.opaque,
          onTap: _toggle,
          child: AnimatedBuilder(
            animation: Listenable.merge([_halo, _anim]),
            builder: (context, _) {
              final active = _anim.value;
              final pulse = 0.55 + (_halo.value * 0.45);
              final restingColor = widget.iconColor ??
                  CelestialColors.textSecondary.withValues(alpha: 0.55);
              final activeColor = widget.accentColor.withValues(alpha: 0.95);
              final iconColor = Color.lerp(restingColor, activeColor, active)!;

              return Padding(
                padding: const EdgeInsets.symmetric(horizontal: 2, vertical: 2),
                child: Container(
                  width: widget.iconSize + 12,
                  height: widget.iconSize + 12,
                  alignment: Alignment.center,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: widget.accentColor.withValues(alpha: 0.06 * active),
                    boxShadow: active > 0
                        ? [
                            BoxShadow(
                              color: widget.accentColor.withValues(
                                alpha: 0.22 * pulse * active,
                              ),
                              blurRadius: 12,
                              spreadRadius: 0.5,
                            ),
                          ]
                        : null,
                  ),
                  child: Icon(
                    Icons.info_outline_rounded,
                    size: widget.iconSize,
                    color: iconColor,
                  ),
                ),
              );
            },
          ),
        ),
      ),
    );
  }

  Widget _buildOverlay(BuildContext context) {
    final placeAbove = _placeAbove;
    final targetAnchor =
        placeAbove ? Alignment.topCenter : Alignment.bottomCenter;
    final followerAnchor =
        placeAbove ? Alignment.bottomCenter : Alignment.topCenter;
    final linkOffset = Offset(0, placeAbove ? -_gap : _gap);

    return Stack(
      children: [
        // Tap-outside dismiss.
        Positioned.fill(
          child: GestureDetector(
            behavior: HitTestBehavior.translucent,
            onTap: _hide,
          ),
        ),
        Positioned(
          left: 0,
          top: 0,
          child: CompositedTransformFollower(
            link: _link,
            showWhenUnlinked: false,
            targetAnchor: targetAnchor,
            followerAnchor: followerAnchor,
            offset: linkOffset,
            child: AnimatedBuilder(
              animation: _anim,
              builder: (context, _) {
                final t =
                    Curves.easeOutCubic.transform(_anim.value.clamp(0.0, 1.0));
                return _animate(t, placeAbove: placeAbove);
              },
            ),
          ),
        ),
      ],
    );
  }

  Widget _animate(double t, {required bool placeAbove}) {
    // Card slides 6px out of place at t=0 and settles into position at t=1.
    final dy = (1 - t) * (placeAbove ? 6.0 : -6.0);
    return IgnorePointer(
      ignoring: t < 0.05,
      child: Opacity(
        opacity: t.clamp(0.0, 1.0),
        child: Transform.translate(
          offset: Offset(0, dy),
          child: Transform.scale(
            scale: 0.965 + 0.035 * t,
            alignment:
                placeAbove ? Alignment.bottomCenter : Alignment.topCenter,
            child: SizedBox(
              width: widget.cardWidth,
              child: _CardSurface(
                accent: widget.accentColor,
                message: widget.message,
                eyebrow: widget.eyebrow,
                contentPadding: widget.contentPadding,
                cardWidth: widget.cardWidth,
                caretPointsDown: placeAbove,
                surfaceTop: _surfaceTop,
                surfaceBottom: _surfaceBottom,
                caretHalfWidth: _caretHalfWidth,
                caretHeight: _caretHeight,
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _CardSurface extends StatefulWidget {
  final Color accent;
  final String message;
  final String? eyebrow;
  final EdgeInsets contentPadding;
  final double cardWidth;
  final bool caretPointsDown;
  final Color surfaceTop;
  final Color surfaceBottom;
  final double caretHalfWidth;
  final double caretHeight;

  const _CardSurface({
    required this.accent,
    required this.message,
    required this.eyebrow,
    required this.contentPadding,
    required this.cardWidth,
    required this.caretPointsDown,
    required this.surfaceTop,
    required this.surfaceBottom,
    required this.caretHalfWidth,
    required this.caretHeight,
  });

  @override
  State<_CardSurface> createState() => _CardSurfaceState();
}

class _CardSurfaceState extends State<_CardSurface>
    with SingleTickerProviderStateMixin {
  late final AnimationController _dotPulse = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 1400),
  )..repeat(reverse: true);

  @override
  void dispose() {
    _dotPulse.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final accent = widget.accent;
    // Caret sits at the card's horizontal midpoint — that's where the
    // trigger's center lives thanks to CompositedTransformFollower's
    // topCenter / bottomCenter anchors.
    final caretCenterX = widget.cardWidth / 2;

    return Material(
      type: MaterialType.transparency,
      child: Stack(
        clipBehavior: Clip.none,
        children: [
          Container(
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(12),
              gradient: LinearGradient(
                begin: Alignment.topLeft,
                end: Alignment.bottomRight,
                colors: [widget.surfaceTop, widget.surfaceBottom],
              ),
              border: Border.all(
                color: accent.withValues(alpha: 0.22),
                width: 1,
              ),
              boxShadow: [
                // Close, near-black drop for depth.
                BoxShadow(
                  color: Colors.black.withValues(alpha: 0.55),
                  blurRadius: 22,
                  offset: const Offset(0, 12),
                ),
                // Soft accent halo for atmosphere.
                BoxShadow(
                  color: accent.withValues(alpha: 0.14),
                  blurRadius: 30,
                  spreadRadius: -6,
                ),
              ],
            ),
            child: ClipRRect(
              borderRadius: BorderRadius.circular(12),
              child: Stack(
                children: [
                  // 1px gradient highlight on the top edge — feels like a
                  // screen edge catching light.
                  Positioned(
                    top: 0,
                    left: 16,
                    right: 16,
                    child: Container(
                      height: 1,
                      decoration: BoxDecoration(
                        gradient: LinearGradient(
                          colors: [
                            Colors.transparent,
                            accent.withValues(alpha: 0.5),
                            Colors.transparent,
                          ],
                        ),
                      ),
                    ),
                  ),
                  Padding(
                    padding: widget.contentPadding,
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        if (widget.eyebrow != null) ...[
                          Row(
                            children: [
                              AnimatedBuilder(
                                animation: _dotPulse,
                                builder: (context, _) {
                                  final v = 0.55 + (_dotPulse.value * 0.45);
                                  return Container(
                                    width: 5,
                                    height: 5,
                                    decoration: BoxDecoration(
                                      shape: BoxShape.circle,
                                      color: accent.withValues(alpha: v),
                                      boxShadow: [
                                        BoxShadow(
                                          color: accent
                                              .withValues(alpha: 0.55 * v),
                                          blurRadius: 6,
                                        ),
                                      ],
                                    ),
                                  );
                                },
                              ),
                              const SizedBox(width: 8),
                              Text(
                                widget.eyebrow!,
                                style: TextStyle(
                                  fontSize: 9.5,
                                  fontWeight: FontWeight.w700,
                                  letterSpacing: 1.6,
                                  color: accent.withValues(alpha: 0.92),
                                ),
                              ),
                            ],
                          ),
                          const SizedBox(height: 7),
                        ],
                        Text(
                          widget.message,
                          style: const TextStyle(
                            color: CelestialColors.textPrimary,
                            fontSize: 12.5,
                            height: 1.42,
                            letterSpacing: 0.1,
                            fontWeight: FontWeight.w400,
                          ),
                        ),
                      ],
                    ),
                  ),
                ],
              ),
            ),
          ),
          // Caret — drawn outside the clipped surface so its stroke isn't
          // cut off.
          Positioned(
            left: caretCenterX - widget.caretHalfWidth,
            top: widget.caretPointsDown ? null : -widget.caretHeight + 0.5,
            bottom: widget.caretPointsDown ? -widget.caretHeight + 0.5 : null,
            child: CustomPaint(
              size: Size(widget.caretHalfWidth * 2, widget.caretHeight),
              painter: _CaretPainter(
                fillColor: widget.caretPointsDown
                    ? widget.surfaceBottom
                    : widget.surfaceTop,
                strokeColor: accent.withValues(alpha: 0.22),
                pointsDown: widget.caretPointsDown,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _CaretPainter extends CustomPainter {
  final Color fillColor;
  final Color strokeColor;
  final bool pointsDown;

  _CaretPainter({
    required this.fillColor,
    required this.strokeColor,
    required this.pointsDown,
  });

  @override
  void paint(Canvas canvas, Size size) {
    final path = Path();
    if (pointsDown) {
      path.moveTo(0, 0);
      path.lineTo(size.width, 0);
      path.lineTo(size.width / 2, size.height);
      path.close();
    } else {
      path.moveTo(0, size.height);
      path.lineTo(size.width, size.height);
      path.lineTo(size.width / 2, 0);
      path.close();
    }
    canvas.drawPath(path, Paint()..color = fillColor);
    // Stroke only the two slanted edges so the caret blends seamlessly with
    // the card body's border.
    final strokePaint = Paint()
      ..color = strokeColor
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1
      ..strokeJoin = StrokeJoin.miter;
    final edges = Path();
    if (pointsDown) {
      edges.moveTo(0, 0);
      edges.lineTo(size.width / 2, size.height);
      edges.lineTo(size.width, 0);
    } else {
      edges.moveTo(0, size.height);
      edges.lineTo(size.width / 2, 0);
      edges.lineTo(size.width, size.height);
    }
    canvas.drawPath(edges, strokePaint);
  }

  @override
  bool shouldRepaint(covariant _CaretPainter old) =>
      old.fillColor != fillColor ||
      old.strokeColor != strokeColor ||
      old.pointsDown != pointsDown;
}
