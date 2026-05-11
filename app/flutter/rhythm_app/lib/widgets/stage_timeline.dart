import 'package:flutter/material.dart';

import 'solar_orbit.dart';

/// One step in a [StageTimeline].
class StageTimelineItem {
  final String label;
  final IconData icon;

  const StageTimelineItem({required this.label, required this.icon});
}

enum _StageState { pending, active, complete, failed, skipped }

/// A vertical "constellation" of progress stages.
///
/// Each stage is a star: pending stars are dim outlines, the active star
/// pulses with a teal corona, completed stars settle into a steady fill, and
/// failed stars flip to a red cross. The connector between stars fills in
/// behind progress like a wick.
class StageTimeline extends StatefulWidget {
  /// All possible stages, in order.
  final List<StageTimelineItem> stages;

  /// Index of the currently-active stage (0-based). Use [stages.length] when
  /// every stage is complete; use -1 when the flow has not started.
  final int activeIndex;

  /// Sub-message shown beneath the active stage (typically the latest
  /// `message` field from a server progress event).
  final String? activeMessage;

  /// Marks the stage at [activeIndex] as failed. Subsequent stages render as
  /// skipped.
  final bool failed;

  /// Optional download progress (0-100) shown inline beneath the active stage
  /// label. Only rendered when [activeIndex] points at a real stage.
  final int? activePercent;

  /// Optional bytes-downloaded label rendered next to the percentage.
  final String? activeBytesLabel;

  /// Accent color (defaults to the app's teal).
  final Color accent;

  const StageTimeline({
    super.key,
    required this.stages,
    required this.activeIndex,
    this.activeMessage,
    this.failed = false,
    this.activePercent,
    this.activeBytesLabel,
    this.accent = const Color(0xFF00BCD4),
  });

  @override
  State<StageTimeline> createState() => _StageTimelineState();
}

class _StageTimelineState extends State<StageTimeline>
    with SingleTickerProviderStateMixin {
  late final AnimationController _pulse;

  @override
  void initState() {
    super.initState();
    _pulse = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 1600),
    )..repeat(reverse: true);
  }

  @override
  void dispose() {
    _pulse.dispose();
    super.dispose();
  }

  _StageState _stateFor(int index) {
    if (widget.failed && widget.activeIndex >= 0) {
      if (index < widget.activeIndex) return _StageState.complete;
      if (index == widget.activeIndex) return _StageState.failed;
      return _StageState.skipped;
    }
    if (index < widget.activeIndex) return _StageState.complete;
    if (index == widget.activeIndex) return _StageState.active;
    return _StageState.pending;
  }

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        for (int i = 0; i < widget.stages.length; i++)
          _StageRow(
            item: widget.stages[i],
            state: _stateFor(i),
            isLast: i == widget.stages.length - 1,
            pulse: _pulse,
            accent: widget.accent,
            subMessage: _stateFor(i) == _StageState.active
                ? widget.activeMessage
                : null,
            percent: _stateFor(i) == _StageState.active
                ? widget.activePercent
                : null,
            bytesLabel: _stateFor(i) == _StageState.active
                ? widget.activeBytesLabel
                : null,
          ),
      ],
    );
  }
}

class _StageRow extends StatelessWidget {
  final StageTimelineItem item;
  final _StageState state;
  final bool isLast;
  final Animation<double> pulse;
  final Color accent;
  final String? subMessage;
  final int? percent;
  final String? bytesLabel;

  const _StageRow({
    required this.item,
    required this.state,
    required this.isLast,
    required this.pulse,
    required this.accent,
    this.subMessage,
    this.percent,
    this.bytesLabel,
  });

  @override
  Widget build(BuildContext context) {
    return IntrinsicHeight(
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: 32,
            child: Column(
              children: [
                SizedBox(
                  height: 28,
                  child: Center(child: _buildDot()),
                ),
                if (!isLast)
                  Expanded(
                    child: _buildConnector(),
                  ),
              ],
            ),
          ),
          Expanded(
            child: Padding(
              padding: EdgeInsets.fromLTRB(8, 4, 0, isLast ? 0 : 16),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      Icon(
                        item.icon,
                        size: 13,
                        color: _labelColor().withValues(alpha: 0.85),
                      ),
                      const SizedBox(width: 7),
                      Flexible(
                        child: Text(
                          item.label,
                          style: TextStyle(
                            color: _labelColor(),
                            fontSize: 14,
                            fontWeight: state == _StageState.active
                                ? FontWeight.w600
                                : FontWeight.w500,
                            letterSpacing: 0.2,
                          ),
                          overflow: TextOverflow.ellipsis,
                        ),
                      ),
                    ],
                  ),
                  if (subMessage != null && subMessage!.trim().isNotEmpty) ...[
                    const SizedBox(height: 5),
                    Padding(
                      padding: const EdgeInsets.only(left: 20),
                      child: Text(
                        subMessage!,
                        style: TextStyle(
                          color: CelestialColors.textSecondary
                              .withValues(alpha: 0.78),
                          fontSize: 12.5,
                          height: 1.45,
                          fontStyle: FontStyle.italic,
                        ),
                      ),
                    ),
                  ],
                  if (percent != null) ...[
                    const SizedBox(height: 8),
                    Padding(
                      padding: const EdgeInsets.only(left: 20),
                      child: _buildProgressBar(),
                    ),
                  ],
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildDot() {
    switch (state) {
      case _StageState.complete:
        return Container(
          width: 18,
          height: 18,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: accent.withValues(alpha: 0.85),
            boxShadow: [
              BoxShadow(
                color: accent.withValues(alpha: 0.25),
                blurRadius: 6,
                spreadRadius: 0.5,
              ),
            ],
          ),
          child: const Icon(
            Icons.check_rounded,
            color: Colors.white,
            size: 12,
          ),
        );

      case _StageState.failed:
        return Container(
          width: 22,
          height: 22,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: const Color(0xFFEF4444),
            boxShadow: [
              BoxShadow(
                color: const Color(0xFFEF4444).withValues(alpha: 0.4),
                blurRadius: 12,
                spreadRadius: 1,
              ),
            ],
          ),
          child: const Icon(
            Icons.close_rounded,
            color: Colors.white,
            size: 14,
          ),
        );

      case _StageState.active:
        return AnimatedBuilder(
          animation: pulse,
          builder: (_, __) {
            final t = pulse.value;
            return SizedBox(
              width: 28,
              height: 28,
              child: Stack(
                alignment: Alignment.center,
                children: [
                  Container(
                    width: 26,
                    height: 26,
                    decoration: BoxDecoration(
                      shape: BoxShape.circle,
                      border: Border.all(
                        color: accent.withValues(alpha: 0.18 + t * 0.25),
                        width: 1,
                      ),
                    ),
                  ),
                  Container(
                    width: 22,
                    height: 22,
                    decoration: BoxDecoration(
                      shape: BoxShape.circle,
                      gradient: RadialGradient(
                        colors: [
                          accent.withValues(alpha: 0.95),
                          accent.withValues(alpha: 0.0),
                        ],
                        stops: const [0.45, 1.0],
                      ),
                      boxShadow: [
                        BoxShadow(
                          color: accent.withValues(alpha: 0.4 + t * 0.4),
                          blurRadius: 14 + t * 8,
                          spreadRadius: 1 + t * 2,
                        ),
                      ],
                    ),
                  ),
                  Container(
                    width: 8,
                    height: 8,
                    decoration: const BoxDecoration(
                      shape: BoxShape.circle,
                      color: Colors.white,
                    ),
                  ),
                ],
              ),
            );
          },
        );

      case _StageState.skipped:
      case _StageState.pending:
        final dim = state == _StageState.skipped ? 0.18 : 0.32;
        return Container(
          width: 14,
          height: 14,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: CelestialColors.backgroundCard,
            border: Border.all(
              color: CelestialColors.textSecondary.withValues(alpha: dim),
              width: 1.2,
            ),
          ),
        );
    }
  }

  Widget _buildConnector() {
    final isFilled = state == _StageState.complete;
    return Container(
      width: 1.5,
      margin: const EdgeInsets.symmetric(vertical: 1),
      decoration: BoxDecoration(
        gradient: LinearGradient(
          begin: Alignment.topCenter,
          end: Alignment.bottomCenter,
          colors: isFilled
              ? [
                  accent.withValues(alpha: 0.7),
                  accent.withValues(alpha: 0.35),
                ]
              : [
                  CelestialColors.orbitRing.withValues(alpha: 0.45),
                  CelestialColors.orbitRing.withValues(alpha: 0.2),
                ],
        ),
      ),
    );
  }

  Widget _buildProgressBar() {
    final pct = (percent ?? 0).clamp(0, 100);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        ClipRRect(
          borderRadius: BorderRadius.circular(3),
          child: Stack(
            children: [
              Container(
                height: 5,
                color: accent.withValues(alpha: 0.08),
              ),
              FractionallySizedBox(
                widthFactor: pct / 100,
                child: Container(
                  height: 5,
                  decoration: BoxDecoration(
                    gradient: LinearGradient(
                      colors: [
                        accent.withValues(alpha: 0.65),
                        accent,
                      ],
                    ),
                  ),
                ),
              ),
            ],
          ),
        ),
        const SizedBox(height: 6),
        Row(
          mainAxisAlignment: MainAxisAlignment.spaceBetween,
          children: [
            if (bytesLabel != null && bytesLabel!.isNotEmpty)
              Text(
                bytesLabel!,
                style: TextStyle(
                  color: CelestialColors.textSecondary.withValues(alpha: 0.62),
                  fontSize: 11.5,
                  fontFamily: 'monospace',
                  letterSpacing: 0.3,
                ),
              )
            else
              const SizedBox.shrink(),
            Text(
              '$pct%',
              style: TextStyle(
                color: accent,
                fontSize: 12,
                fontWeight: FontWeight.w600,
                fontFamily: 'monospace',
                letterSpacing: 0.4,
              ),
            ),
          ],
        ),
      ],
    );
  }

  Color _labelColor() {
    switch (state) {
      case _StageState.active:
        return CelestialColors.textPrimary;
      case _StageState.complete:
        return CelestialColors.textPrimary.withValues(alpha: 0.85);
      case _StageState.failed:
        return const Color(0xFFEF4444);
      case _StageState.pending:
        return CelestialColors.textSecondary.withValues(alpha: 0.55);
      case _StageState.skipped:
        return CelestialColors.textSecondary.withValues(alpha: 0.32);
    }
  }
}
